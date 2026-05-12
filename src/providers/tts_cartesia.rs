use super::Tts;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;

const DEFAULT_VOICE_ID: &str = "a0e99841-438c-4a64-b679-ae501e7d6091"; // Barbershop Man
const MODEL_ID: &str = "sonic-2";

/// Sample rate used for the streaming PCM output. Match this when constructing
/// rodio SamplesBuffers on the consumer side.
pub const STREAM_SAMPLE_RATE: u32 = 24000;
pub const STREAM_CHANNELS: u16 = 1;

pub struct TtsCartesia {
    pub api_key: String,
    pub voice_id: String,
}

impl TtsCartesia {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("CARTESIA_API_KEY")?;
        let voice_id =
            std::env::var("CARTESIA_VOICE_ID").unwrap_or_else(|_| DEFAULT_VOICE_ID.to_string());
        Ok(TtsCartesia { api_key, voice_id })
    }
}

impl Tts for TtsCartesia {
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model_id": MODEL_ID,
            "transcript": text,
            "voice": { "mode": "id", "id": self.voice_id },
            "output_format": {
                "container": "wav",
                "encoding": "pcm_s16le",
                "sample_rate": 24000,
            },
            "language": "en",
        });

        let response = reqwest::blocking::Client::new()
            .post("https://api.cartesia.ai/tts/bytes")
            .bearer_auth(&self.api_key)
            .header("Cartesia-Version", "2026-03-01")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Cartesia TTS error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        Ok(response.bytes()?.to_vec())
    }
}

impl TtsCartesia {
    /// Streaming TTS — POSTs to `/tts/sse` with raw PCM output and fires
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

        let response = reqwest::Client::new()
            .post("https://api.cartesia.ai/tts/sse")
            .bearer_auth(&self.api_key)
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
                    if event["type"] == "chunk" {
                        if let Some(b64) = event["data"].as_str() {
                            if let Ok(pcm) = BASE64.decode(b64) {
                                on_chunk(&pcm);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
