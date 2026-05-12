#[path = "../screenshot.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::Tts;

fn main() {
    let tts = providers::tts_openai::TtsOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");

    println!("synthesizing...");
    let mp3 = tts
        .synthesize("Hello from aegis. The voice loop is alive.")
        .expect("TTS failed");
    println!("got {} bytes of MP3", mp3.len());

    std::fs::write("/tmp/aegis-tts.mp3", &mp3).expect("write failed");
    println!("saved /tmp/aegis-tts.mp3, playing...");

    let file = std::io::BufReader::new(
        std::fs::File::open("/tmp/aegis-tts.mp3").expect("could not open mp3"),
    );
    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .expect("could not open audio output");
    let player = rodio::play(handle.mixer(), file).expect("could not play");
    player.sleep_until_end();
}
