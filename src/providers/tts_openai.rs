use super::Tts;

pub struct TtsOpenAi {
    pub api_key: String,
}

impl TtsOpenAi {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("OPENAI_API_KEY")?;
        Ok(TtsOpenAi { api_key })
    }
}

impl Tts for TtsOpenAi {
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model": "gpt-4o-mini-tts",
            "voice": "alloy",
            "input": text,
        });

        let response = reqwest::blocking::Client::new()
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "OpenAI TTS error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        Ok(response.bytes()?.to_vec())
    }
}
