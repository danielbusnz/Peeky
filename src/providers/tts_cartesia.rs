use super::Tts;

const DEFAULT_VOICE_ID: &str = "a0e99841-438c-4a64-b679-ae501e7d6091"; // Barbershop Man
const MODEL_ID: &str = "sonic-2";

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
