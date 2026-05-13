#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey/mod.rs"]
mod hotkey;
#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::Stt;

fn main() {
    let whisper = providers::whisper_openai::WhisperOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");

    hotkey::init().expect("signal handler setup");
    println!("hold SUPER+space to record, release to transcribe (Ctrl+C to quit)");

    loop {
        hotkey::wait_for_press();
        let (samples, sample_rate, channels) = audio::record_until_release();
        println!(
            "captured {} samples ({}Hz, {}ch), sending to Whisper...",
            samples.len(),
            sample_rate,
            channels
        );

        match whisper.transcribe(&samples, sample_rate, channels) {
            Ok(transcript) => println!("transcript: {}\n", transcript),
            Err(e) => eprintln!("transcription failed: {}\n", e),
        }
    }
}
