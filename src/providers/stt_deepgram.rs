use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Streaming Speech-to-Text via Deepgram's WebSocket endpoint.
///
/// Audio chunks (i16 PCM, little-endian) arrive via the channel; the
/// transcript builds up from interim/final segments and is returned when
/// the audio sender is dropped (end of recording).
pub struct SttDeepgram {
    pub api_key: String,
}

impl SttDeepgram {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("DEEPGRAM_API_KEY")?;
        Ok(Self { api_key })
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
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Build the WSS URL with query params. Deepgram expects the audio
        // format declared here to match what we send.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language=en\
             &encoding=linear16&sample_rate={}&channels={}\
             &punctuate=true&interim_results=false&smart_format=true",
            sample_rate, channels
        );

        // Build the WS request via tungstenite's IntoClientRequest, then
        // attach the Authorization header. tokio-tungstenite auto-fills the
        // mandatory handshake headers (Sec-WebSocket-Key, Upgrade, etc.).
        let mut request = url.into_client_request()?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {}", self.api_key).parse()?,
        );

        let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut write, mut read) = ws_stream.split();

        // Task 1: pump audio chunks from the channel into WS binary frames.
        let send_task = tokio::spawn(async move {
            while let Some(samples) = audio_rx.recv().await {
                // Convert i16 samples to little-endian bytes.
                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                if write.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
            // Audio sender dropped. Send Finalize first to force Deepgram
            // to commit any pending audio as a final transcript, then
            // CloseStream to signal end-of-session, then close the WS.
            let _ = write
                .send(Message::Text("{\"type\":\"Finalize\"}".to_string().into()))
                .await;
            let _ = write
                .send(Message::Text(
                    "{\"type\":\"CloseStream\"}".to_string().into(),
                ))
                .await;
            let _ = write.close().await;
        });

        // Task 2: read transcript JSON frames from Deepgram.
        let mut transcript = String::new();
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    if event["type"] == "Results" {
                        // Only accumulate final transcripts (interim_results=false
                        // means we don't get partials, but is_final still gates).
                        if event["is_final"].as_bool().unwrap_or(false) {
                            if let Some(t) =
                                event["channel"]["alternatives"][0]["transcript"].as_str()
                            {
                                if !t.is_empty() {
                                    if !transcript.is_empty() {
                                        transcript.push(' ');
                                    }
                                    transcript.push_str(t);
                                }
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }

        let _ = send_task.await;
        Ok(transcript)
    }
}
