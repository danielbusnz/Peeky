#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::Tts;

fn main() {
    let tts = providers::tts_cartesia::TtsCartesia::from_env()
        .expect("missing CARTESIA_API_KEY");

    println!("synthesizing via Cartesia...");
    let wav = tts
        .synthesize("Hello from aegis. This is Cartesia speaking.")
        .expect("Cartesia failed");
    println!("got {} bytes of WAV", wav.len());

    std::fs::write("/tmp/aegis-cartesia.wav", &wav).expect("write failed");
    println!("saved /tmp/aegis-cartesia.wav, playing...");

    let file = std::io::BufReader::new(
        std::fs::File::open("/tmp/aegis-cartesia.wav").expect("could not open wav"),
    );
    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .expect("could not open audio output");
    let player = rodio::play(handle.mixer(), file).expect("could not play");
    player.sleep_until_end();
}
