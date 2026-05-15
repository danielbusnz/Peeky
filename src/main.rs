mod audio;
mod cursor;
mod hotkey;
mod mouse;
#[cfg(feature = "hyprland")]
mod painter;
mod providers;
mod screenshot;
mod voice;

fn main() {
    // One reqwest::Client shared across all HTTP providers. Internal Arc
    // means clones share the same connection pool — TLS sessions get reused
    // across calls, HTTP/2 multiplexes our parallel Claude calls onto one
    // TCP connection, and there's no per-call handshake cost after the first.
    let http = reqwest::Client::new();

    let stt = providers::stt_deepgram::SttDeepgram::from_env(http.clone())
        .expect("STT init failed");
    let claude =
        providers::claude::Claude::from_env(http.clone()).expect("Claude provider init failed");
    let cartesia = providers::tts_cartesia::TtsCartesia::from_env(http)
        .expect("missing CARTESIA_API_KEY");
    let mic = audio::Mic::init();

    hotkey::init().expect("signal handler setup");

    std::thread::spawn(move || voice::run_loop(mic, stt, claude, cartesia));

    cursor::cursor(300, 300);
}
