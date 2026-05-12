use super::Stt;

pub struct WhisperOpenAi {
    pub api_key: String,
}

impl WhisperOpenAi {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("OPENAI_API_KEY")?;
        Ok(WhisperOpenAi { api_key })
    }
}

impl Stt for WhisperOpenAi {
    fn transcribe(
        &self,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let wav_bytes = encode_wav(samples, sample_rate, channels)?;

        let form = reqwest::blocking::multipart::Form::new()
            .part(
                "file",
                reqwest::blocking::multipart::Part::bytes(wav_bytes)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?,
            )
            .text("model", "whisper-1")
            .text("response_format", "text");

        let response = reqwest::blocking::Client::new()
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Whisper error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        Ok(response.text()?.trim().to_string())
    }
}

fn encode_wav(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut buf, spec)?;
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let pcm16 = (clamped * i16::MAX as f32) as i16;
            writer.write_sample(pcm16)?;
        }
        writer.finalize()?;
    }
    Ok(buf.into_inner())
}
