#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey.rs"]
mod hotkey;
#[path = "../screenshot.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::Stt;

fn main() {
    let whisper = providers::whisper_openai::WhisperOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");

    hotkey::init().expect("signal handler setup");
    println!("press SUPER+space to record, release to stop...");
    hotkey::wait_for_press();

    let (samples, sample_rate, channels) = audio::record_until_release();
    println!(
        "captured {} samples ({}Hz, {}ch), sending to Whisper...",
        samples.len(),
        sample_rate,
        channels
    );

    let transcript = whisper
        .transcribe(&samples, sample_rate, channels)
        .expect("transcription failed");

    println!("transcript: {}", transcript);
}
