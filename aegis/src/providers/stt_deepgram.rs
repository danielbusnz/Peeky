//! Deepgram streaming STT over WebSocket. Audio chunks flow in via an
//! mpsc channel, transcripts build up from interim/final events, and
//! the final string returns when the upstream sender is dropped (i.e.
//! the hotkey is released).
//!
//! Tail handling: Deepgram fires interim transcripts during speech and
//! emits is_final events after pauses. Release-to-transcript-ready
//! latency depends on STT_QUIESCENCE_MS (tuning.rs).

use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const PROXY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/deepgram/token";

/// Streaming Speech-to-Text via Deepgram's WebSocket endpoint.
///
/// Audio chunks (i16 PCM, little-endian) arrive via the channel; the
/// transcript builds up from interim/final segments and is returned when
/// the audio sender is dropped (end of recording).
#[derive(Clone)]
pub struct SttDeepgram {
    pub http: reqwest::Client,
    pub mode: SttMode,
}

/// Auth mode. In proxy mode aegis fetches a short-lived JWT from aegis-proxy
/// before opening Deepgram's WebSocket. In direct mode the local API key is
/// used as a `Token`-prefixed credential. Only enable with
/// `AEGIS_DEEPGRAM_DIRECT=1` (useful for dev / burning your own quota).
#[derive(Clone)]
pub enum SttMode {
    Direct {
        api_key: String,
    },
    Proxy {
        token_url: String,
        device_id: String,
    },
}

impl SttDeepgram {
    /// Initialize from `.env`/environment. Default behavior is to route auth
    /// through aegis-proxy (no Deepgram key needed locally). Set
    /// `AEGIS_DEEPGRAM_DIRECT=1` + provide `DEEPGRAM_API_KEY` to bypass.
    pub fn from_env(http: reqwest::Client) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();

        if std::env::var("AEGIS_DEEPGRAM_DIRECT").is_ok() {
            let api_key = std::env::var("DEEPGRAM_API_KEY")?;
            return Ok(Self {
                http,
                mode: SttMode::Direct { api_key },
            });
        }

        let device_id = super::device_id::load_or_create()?;
        Ok(Self {
            http,
            mode: SttMode::Proxy {
                token_url: PROXY_URL.to_string(),
                device_id,
            },
        })
    }

    /// Pre-open HTTPS connections so the first real STT session doesn't
    /// pay TCP+TLS handshake cost. Warms both api.deepgram.com and the
    /// proxy auth endpoint (if using proxy mode). In proxy mode we do a
    /// real auth call (with device_id) to fully warm the path; the token
    /// is discarded since it's short-lived.
    pub async fn warm(&self) {
        // Warm Deepgram API connection
        let dg = self
            .http
            .get("https://api.deepgram.com/v1/projects")
            .header("Authorization", "Token warm")
            .send();

        // In proxy mode, do a real auth call to fully warm the path
        match &self.mode {
            SttMode::Proxy {
                token_url,
                device_id,
            } => {
                let mut req = self
                    .http
                    .post(token_url)
                    .header(super::proxy_contract::DEVICE_ID_HEADER, device_id);
                if let Some(code) = super::invite_code::load() {
                    req = req.header(super::proxy_contract::INVITE_CODE_HEADER, code);
                }
                let proxy = req.send();
                let _ = tokio::join!(dg, proxy);
            }
            SttMode::Direct { .. } => {
                let _ = dg.await;
            }
        }
    }

    /// Build the `Authorization` header value for the Deepgram WebSocket.
    /// In direct mode the API key is used as-is with a `Token` prefix.
    /// In proxy mode we POST to aegis-proxy for a short-lived JWT and use
    /// `Bearer <jwt>` (Deepgram's token-based auth scheme).
    async fn auth_header(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match &self.mode {
            SttMode::Direct { api_key } => Ok(format!("Token {}", api_key)),
            SttMode::Proxy {
                token_url,
                device_id,
            } => {
                // Re-read the invite code on every mint so the onboarding
                // window can change it without an aegis restart.
                let mut req = self
                    .http
                    .post(token_url)
                    .header(super::proxy_contract::DEVICE_ID_HEADER, device_id);
                if let Some(code) = super::invite_code::load() {
                    req = req.header(super::proxy_contract::INVITE_CODE_HEADER, code);
                }
                let resp = req.send().await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(format!("deepgram token mint failed {}: {}", status, body).into());
                }
                let json: serde_json::Value = resp.json().await?;
                let token = json["token"]
                    .as_str()
                    .ok_or("deepgram token response missing 'token' field")?
                    .to_string();
                Ok(format!("Bearer {}", token))
            }
        }
    }

    /// Open a WebSocket session, pump audio chunks from `audio_rx`, return
    /// the final transcript when the audio channel closes.
    ///
    /// The audio is expected to be `linear16` PCM at the given sample rate
    /// and channel count.
    pub async fn transcribe_stream(
        &self,
        sample_rate: u32,
        channels: u16,
        mut audio_rx: UnboundedReceiver<Vec<i16>>,
        interim_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Build the WSS URL with query params. Deepgram expects the audio
        // format declared here to match what we send. interim_results=true
        // lets us return the latest partial transcript the moment the user
        // releases, without waiting for Deepgram's final commit.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language=en\
             &encoding=linear16&sample_rate={}&channels={}\
             &punctuate=true&interim_results=true&smart_format=true",
            sample_rate, channels
        );

        // Resolve the auth header. In proxy mode this hits aegis-proxy for a
        // short-lived JWT first (~50ms HTTPS round-trip). In direct mode it's
        // an in-memory string lookup.
        let t_auth = std::time::Instant::now();
        let auth = self.auth_header().await?;
        eprintln!("[deepgram-debug] auth_header → {:?}", t_auth.elapsed());

        // Build the WS request via tungstenite's IntoClientRequest, then
        // attach the Authorization header. tokio-tungstenite auto-fills the
        // mandatory handshake headers (Sec-WebSocket-Key, Upgrade, etc.).
        let mut request = url.into_client_request()?;
        request.headers_mut().insert("Authorization", auth.parse()?);

        let t_connect = std::time::Instant::now();
        let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
        eprintln!("[deepgram-debug] ws connect → {:?}", t_connect.elapsed());
        let (mut write, mut read) = ws_stream.split();

        // Oneshot signal: fires the moment audio_rx closes (user released
        // the hotkey). The read loop uses this to return immediately with
        // whatever transcript it has, instead of waiting for is_final.
        let (release_tx, mut release_rx) = tokio::sync::oneshot::channel::<()>();

        // Task 1: pump audio chunks into WS. When audio_rx closes, send
        // Finalize so Deepgram commits any pending audio as an is_final
        // event. Fire the release signal so the read loop enters its
        // "wait for is_final" phase. Then send CloseStream + close.
        let _send_task = tokio::spawn(async move {
            while let Some(samples) = audio_rx.recv().await {
                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                if write.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
            // Audio EOS. Tell Deepgram to commit pending audio as is_final.
            let _ = write
                .send(Message::Text("{\"type\":\"Finalize\"}".to_string().into()))
                .await;
            // Signal the read loop to switch from live to "await is_final".
            let _ = release_tx.send(());
            // CRITICAL: do NOT send CloseStream or close the WS yet, or
            // Deepgram will hang up before emitting the is_final event we
            // just asked for via Finalize. Wait until the read loop has
            // had time to receive the response, then clean up.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let _ = write
                .send(Message::Text(
                    "{\"type\":\"CloseStream\"}".to_string().into(),
                ))
                .await;
            let _ = write.close().await;
        });

        // Task 2: read transcripts. Track committed is_final segments
        // separately from the latest interim guess.
        //
        // Phase 1 (recording): race release signal against incoming frames.
        // Phase 2 (await final): after release, send_task issued a Finalize
        // to Deepgram. Keep processing frames until we see an `is_final`
        // covering the tail audio, OR until POST_RELEASE_TIMEOUT_MS as a
        // safety net.
        let mut finalized = String::new();
        let mut latest_interim = String::new();

        // Track what we last broadcast so we only ping the interim_tx on
        // actual changes (avoids spamming the speculative watchdog).
        let mut last_broadcast = String::new();

        // Phase 1: live forwarding.
        loop {
            tokio::select! {
                biased;
                _ = &mut release_rx => break,
                msg = read.next() => {
                    match process_frame(msg, &mut finalized, &mut latest_interim) {
                        FrameOutcome::Continue | FrameOutcome::GotFinal => {
                            // Broadcast the running merged transcript whenever
                            // it changes, so the speculative watchdog (if any)
                            // can detect stability.
                            if let Some(ref tx) = interim_tx {
                                let current = merge_ref(&finalized, &latest_interim);
                                if current != last_broadcast {
                                    last_broadcast = current.clone();
                                    let _ = tx.send(current);
                                }
                            }
                        }
                        FrameOutcome::WsClosed => {
                            // WS died mid-stream. Bail with what we have.
                            let result = merge(finalized, latest_interim);
                            eprintln!("[deepgram-debug] WS closed mid-stream, returning: {:?}", result);
                            return Ok(result);
                        }
                    }
                }
            }
        }

        // Phase 2: wait for Deepgram's tail is_final events.
        //
        // Deepgram can split a single utterance into multiple is_final
        // segments. Breaking on the first non-empty final clips speech like
        // "what is your" + "name". Instead: after content arrives, watch
        // for QUIESCENCE_MS of stream silence to detect "Deepgram is done",
        // bounded by POST_RELEASE_TIMEOUT_MS as a hard ceiling.
        eprintln!(
            "[deepgram-debug] released, awaiting is_final (timeout {}ms)...",
            POST_RELEASE_TIMEOUT_MS
        );
        let outer_deadline = Instant::now() + Duration::from_millis(POST_RELEASE_TIMEOUT_MS);
        loop {
            let remaining = outer_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                eprintln!("[deepgram-debug] is_final timeout reached");
                break;
            }
            // Once we have content, only wait QUIESCENCE_MS for stragglers.
            // While still empty, wait up to the full remaining budget.
            let read_budget = if finalized.is_empty() {
                remaining
            } else {
                remaining.min(Duration::from_millis(QUIESCENCE_MS))
            };
            match tokio::time::timeout(read_budget, read.next()).await {
                Err(_) => {
                    if !finalized.is_empty() {
                        eprintln!(
                            "[deepgram-debug] quiescence after final ({}ms)",
                            QUIESCENCE_MS
                        );
                        break;
                    }
                    // No content yet; loop continues until outer deadline.
                }
                Ok(msg) => match process_frame(msg, &mut finalized, &mut latest_interim) {
                    FrameOutcome::GotFinal => {
                        if finalized.is_empty() {
                            eprintln!("[deepgram-debug] empty FINAL, still waiting...");
                        }
                    }
                    FrameOutcome::Continue => {}
                    FrameOutcome::WsClosed => break,
                },
            }
        }

        let result = merge(finalized, latest_interim);
        eprintln!("[deepgram-debug] returning: {:?}", result);
        Ok(result)
    }
}

/// Max time to wait after release for Deepgram's is_final response.
/// Deepgram normally answers within 100-300ms of the Finalize message.
/// If something goes wrong (network blip, Deepgram delay), we still return
/// after this timeout with whatever interim we have, so the user isn't
/// stuck.
const POST_RELEASE_TIMEOUT_MS: u64 = 1200;

use crate::tuning::STT_QUIESCENCE_MS as QUIESCENCE_MS;

/// What happened when we processed a frame.
enum FrameOutcome {
    /// Frame processed (interim or non-Results), keep looping.
    Continue,
    /// Frame was an is_final event. Caller may want to stop waiting.
    GotFinal,
    /// The WS stream ended. Caller must stop.
    WsClosed,
}

/// Process one Deepgram WebSocket frame. Updates `finalized` and
/// `latest_interim` in place, and reports what happened.
fn process_frame(
    msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    finalized: &mut String,
    latest_interim: &mut String,
) -> FrameOutcome {
    let Some(Ok(Message::Text(text))) = msg else {
        return FrameOutcome::WsClosed;
    };
    let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
        return FrameOutcome::Continue;
    };
    if event["type"] != "Results" {
        return FrameOutcome::Continue;
    }
    let Some(t) = event["channel"]["alternatives"][0]["transcript"].as_str() else {
        return FrameOutcome::Continue;
    };
    let is_final = event["is_final"].as_bool().unwrap_or(false);
    eprintln!(
        "[deepgram-debug] {} → {:?}",
        if is_final { "FINAL" } else { "interim" },
        t
    );
    if is_final {
        if !t.is_empty() {
            if !finalized.is_empty() {
                finalized.push(' ');
            }
            finalized.push_str(t);
        }
        latest_interim.clear();
        FrameOutcome::GotFinal
    } else {
        *latest_interim = t.to_string();
        FrameOutcome::Continue
    }
}

fn merge(finalized: String, latest_interim: String) -> String {
    if latest_interim.is_empty() {
        return finalized;
    }
    if finalized.is_empty() {
        return latest_interim;
    }
    format!("{} {}", finalized, latest_interim)
}

/// Same as `merge` but borrows its inputs. Used by the running-transcript
/// broadcast inside the read loop where we don't want to consume state.
fn merge_ref(finalized: &str, latest_interim: &str) -> String {
    if latest_interim.is_empty() {
        return finalized.to_string();
    }
    if finalized.is_empty() {
        return latest_interim.to_string();
    }
    format!("{} {}", finalized, latest_interim)
}
