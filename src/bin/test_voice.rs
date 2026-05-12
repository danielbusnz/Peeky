#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey.rs"]
mod hotkey;
#[path = "../screenshot.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::{Llm, Stt, Tts};

fn main() {
    let whisper = providers::whisper_openai::WhisperOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");
    let claude = providers::claude::Claude::from_env()
        .expect("missing ANTHROPIC_API_KEY");
    let cartesia = providers::tts_cartesia::TtsCartesia::from_env()
        .expect("missing CARTESIA_API_KEY");

    hotkey::init().expect("signal handler setup");
    println!("press SUPER+space to record, release to stop...");
    hotkey::wait_for_press();

    let (samples, sample_rate, channels) = audio::record_until_release();
    println!("recorded {} samples, transcribing...", samples.len());

    let transcript = whisper
        .transcribe(&samples, sample_rate, channels)
        .expect("transcription failed");
    println!("you said: {}", transcript);

    println!("asking claude...");
    let reply = claude.complete(&transcript).expect("claude failed");
    println!("claude says: {}", reply);

    println!("synthesizing reply...");
    let wav = cartesia.synthesize(&reply).expect("Cartesia failed");
    std::fs::write("/tmp/aegis-reply.wav", &wav).expect("write failed");

    println!("playing...");
    let file = std::io::BufReader::new(
        std::fs::File::open("/tmp/aegis-reply.wav").expect("could not open wav"),
    );
    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .expect("could not open audio output");
    let player = rodio::play(handle.mixer(), file).expect("could not play");
    player.sleep_until_end();
}
