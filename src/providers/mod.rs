pub mod claude;
pub mod tts_openai;
pub mod whisper_openai;

pub trait Llm {
    fn complete(&self, prompt: &str) -> Result<String, Box<dyn std::error::Error>>;
}

pub trait Tts {
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>>;
}

pub trait Stt {
    fn transcribe(
        &self,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<String, Box<dyn std::error::Error>>;
}
