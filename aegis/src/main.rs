mod actions;
mod audio;
mod barge_in;
mod ai_cursor;
mod hotkey;
mod integrations;
mod intent;
mod mouse_position;
#[cfg(any(feature = "hyprland", feature = "winit-window"))]
mod painter;
mod providers;
mod screenshot;
mod tuning;
mod voice_session;
mod orchestrator;

fn main() {
    // One reqwest::Client shared across all HTTP providers. Internal Arc
    // means clones share the same connection pool — TLS sessions get reused
    // across calls, HTTP/2 multiplexes our parallel Claude calls onto one
    // TCP connection, and there's no per-call handshake cost after the first.
    let http = reqwest::Client::new();

    let stt =
        providers::stt_deepgram::SttDeepgram::from_env(http.clone()).expect("STT init failed");
    let claude =
        providers::claude::Claude::from_env(http.clone()).expect("Claude provider init failed");
    let cartesia =
        providers::tts_cartesia::TtsCartesia::from_env(http).expect("missing CARTESIA_API_KEY");
    let mic = audio::Mic::init();
    actions::init_input_executor();
    actions::check_input_injection_available();

    // Let the cursor overlay's Soundwave read live mic RMS.
    #[cfg(any(feature = "hyprland", feature = "winit-window"))]
    painter::set_audio_level_source(|| {
        f32::from_bits(audio::AUDIO_LEVEL.load(std::sync::atomic::Ordering::Relaxed))
    });

    hotkey::init().expect("signal handler setup");

    // Probe every integration API in parallel on a background thread so the
    // cursor opens immediately. Pure diagnostics — never gates boot.
    std::thread::spawn(integrations::health::check_and_print);

    std::thread::spawn(move || orchestrator::run_loop(mic, stt, claude, cartesia));

    ai_cursor::cursor(300, 300);
}
