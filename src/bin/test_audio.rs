#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey.rs"]
mod hotkey;

fn main() {
    println!("--- playback test ---");
    play_test();

    println!("--- record test ---");
    hotkey::init().expect("signal handler setup");
    println!("press SUPER+space to record, release to stop...");
    hotkey::wait_for_press();
    let (samples, sample_rate, channels) = audio::record_until_release();
    println!(
        "captured {} samples ({}Hz, {}ch)",
        samples.len(),
        sample_rate,
        channels
    );
    if samples.len() >= 10 {
        let mid = samples.len() / 2;
        println!("middle samples: {:?}", &samples[mid..mid + 10]);
    }
}

fn play_test() {
    let file = std::io::BufReader::new(
        std::fs::File::open("/usr/share/sounds/alsa/Front_Center.wav")
            .expect("could not open sound file"),
    );
    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .expect("could not open audio output");
    let player = rodio::play(handle.mixer(), file).expect("could not play");
    player.sleep_until_end();
}
