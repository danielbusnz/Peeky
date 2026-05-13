#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey/mod.rs"]
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
    let bytes = std::fs::read("/usr/share/sounds/alsa/Front_Center.wav")
        .expect("could not read sound file");
    audio::play(&bytes).expect("playback failed");
}
