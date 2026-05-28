//! Cartesia TTS via SSE. POSTs the text to be spoken and streams back
//! raw PCM chunks; the caller pipes them into rodio as they arrive so
//! speech starts before the full audio is synthesized.
//!
//! Two auth modes: proxy (default, no API key on disk) and direct
//! (`AEGIS_CARTESIA_DIRECT=1` + `CARTESIA_API_KEY`).

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;

/// Voice fallback when CARTESIA_VOICE_ID isn't set. "Barbershop Man" is
/// a calm masculine voice that reads aegis's terse replies well.
const DEFAULT_VOICE_ID: &str = "a0e99841-438c-4a64-b679-ae501e7d6091";
const MODEL_ID: &str = "sonic-2";
const PROXY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/cartesia/token";

/// PCM sample rate of the streamed audio. Consumers must construct
/// rodio SamplesBuffers with this exact rate or speech plays at the
/// wrong pitch.
pub const STREAM_SAMPLE_RATE: u32 = 24000;

/// PCM channel count of the streamed audio. Mono only; multi-channel
/// would just duplicate samples and waste bandwidth.
pub const STREAM_CHANNELS: u16 = 1;

#[derive(Clone)]
pub struct TtsCartesia {
    pub voice_id: String,
    pub http: reqwest::Client,
    pub mode: TtsMode,
}

/// Auth mode. Default routes through aegis-proxy (no Cartesia key locally).
/// Set `AEGIS_CARTESIA_DIRECT=1` + provide `CARTESIA_API_KEY` to bypass.
#[derive(Clone)]
pub enum TtsMode {
    Direct {
        api_key: String,
    },
    Proxy {
        token_url: String,
        device_id: String,
    },
}

impl TtsCartesia {
    /// Loads voice config from env and decides whether to mint tokens via
    /// aegis-proxy (default) or use a local API key (AEGIS_CARTESIA_DIRECT=1).
    /// Takes a shared `reqwest::Client` so subsequent calls reuse TLS.
    pub fn from_env(http: reqwest::Client) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let voice_id =
            std::env::var("CARTESIA_VOICE_ID").unwrap_or_else(|_| DEFAULT_VOICE_ID.to_string());

        let mode = if std::env::var("AEGIS_CARTESIA_DIRECT").is_ok() {
            let api_key = std::env::var("CARTESIA_API_KEY")?;
            eprintln!("[tts-cartesia] mode=Direct (using CARTESIA_API_KEY)");
            TtsMode::Direct { api_key }
        } else {
            let device_id = super::device_id::load_or_create()?;
            eprintln!("[tts-cartesia] mode=Proxy (set AEGIS_CARTESIA_DIRECT=1 to bypass)");
            TtsMode::Proxy {
                token_url: PROXY_URL.to_string(),
                device_id,
            }
        };

        Ok(TtsCartesia {
            voice_id,
            http,
            mode,
        })
    }

    /// Pre-open the HTTPS connection to api.cartesia.ai so the first real
    /// synthesis request doesn't pay TLS handshake cost.
    pub async fn warm(&self) {
        let _ = self
            .http
            .get("https://api.cartesia.ai/voices/")
            .header("X-API-Key", "warm")
            .header("Cartesia-Version", "2026-03-01")
            .send()
            .await;
    }

    /// Returns the Bearer token to send to Cartesia. In direct mode it's the
    /// raw API key. In proxy mode it's a short-lived JWT minted by aegis-proxy
    /// on each call (~50ms HTTPS round-trip per synthesis). Per-call (vs. per-
    /// turn) minting is wasteful when a turn produces many sentences; revisit
    /// if Cartesia daily caps trip in practice.
    async fn bearer_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match &self.mode {
            TtsMode::Direct { api_key } => Ok(api_key.clone()),
            TtsMode::Proxy {
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
                    return Err(format!("cartesia token mint failed {}: {}", status, body).into());
                }
                let json: serde_json::Value = resp.json().await?;
                let token = json["token"]
                    .as_str()
                    .ok_or("cartesia token response missing 'token' field")?
                    .to_string();
                Ok(token)
            }
        }
    }
}

impl TtsCartesia {
    /// Streaming TTS. POSTs to `/tts/sse` with raw PCM output and fires
    /// `on_chunk` for each audio chunk as it arrives. Caller is expected to
    /// pipe the chunks into rodio (or any sink that accepts raw i16 PCM at
    /// 24kHz mono).
    ///
    /// First chunk typically arrives ~150-300ms after the request is sent.
    pub async fn synthesize_stream<F>(
        &self,
        text: &str,
        mut on_chunk: F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&[u8]),
    {
        let body = serde_json::json!({
            "model_id": MODEL_ID,
            "transcript": text,
            "voice": { "mode": "id", "id": self.voice_id },
            "output_format": {
                "container": "raw",
                "encoding": "pcm_s16le",
                "sample_rate": STREAM_SAMPLE_RATE,
            },
            "language": "en",
        });

        let token = self.bearer_token().await?;
        let response = self
            .http
            .post("https://api.cartesia.ai/tts/sse")
            .bearer_auth(&token)
            .header("Cartesia-Version", "2026-03-01")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Cartesia TTS error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            // Cartesia uses standard SSE: data: <json>\n\n per event.
            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    if event["type"] == "chunk"
                        && let Some(b64) = event["data"].as_str()
                        && let Ok(pcm) = BASE64.decode(b64)
                    {
                        on_chunk(&pcm);
                    }
                }
            }
        }

        Ok(())
    }
}
