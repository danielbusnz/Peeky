#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey.rs"]
mod hotkey;
#[path = "../screenshot.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::{Stt, Tts};

fn main() {
    let whisper = providers::whisper_openai::WhisperOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");
    let claude = providers::claude::Claude::from_env()
        .expect("missing ANTHROPIC_API_KEY");
    let cartesia = providers::tts_cartesia::TtsCartesia::from_env()
        .expect("missing CARTESIA_API_KEY");

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    hotkey::init().expect("signal handler setup");
    println!("press SUPER+space to record, release to stop...");
    hotkey::wait_for_press();

    let (samples, sample_rate, channels) = audio::record_until_release();
    println!("recorded {} samples, transcribing...", samples.len());

    let transcript = whisper
        .transcribe(&samples, sample_rate, channels)
        .expect("transcription failed");
    println!("you said: {}", transcript);

    println!("asking claude (streaming)...");
    print!("claude: ");
    let reply = rt
        .block_on(async {
            claude
                .complete_stream(&transcript, |token| {
                    print!("{}", token);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                })
                .await
        })
        .expect("claude failed");
    println!("\n");

    println!("synthesizing reply...");
    let wav = cartesia.synthesize(&reply).expect("Cartesia failed");

    println!("playing...");
    audio::play(&wav).expect("playback failed");
}
