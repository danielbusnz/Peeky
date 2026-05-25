use aegis::{actions, ai_cursor, audio, hotkey, integrations, orchestrator, painter, providers, screenshot};

fn main() {
    // Shared reqwest::Client. Internal Arc means clones reuse the same
    // connection pool: TLS sessions, HTTP/2 multiplexing, and no per-call
    // handshake cost after the first.
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

    // Trigger screen recording permission prompt on macOS by attempting a capture.
    // Microphone permission is already triggered by audio::Mic::init() above.
    // This runs early so the permission dialogs don't interrupt the voice flow.
    #[cfg(target_os = "macos")]
    {
        let _ = screenshot::capture_for_claude(0, 0, 100, 100);
        eprintln!("[startup] screen recording permission check triggered");
    }

    // Wire the soundwave painter to live mic RMS so the overlay reflects
    // input level without an explicit per-frame channel.
    painter::set_audio_level_source(|| {
        f32::from_bits(audio::AUDIO_LEVEL.load(std::sync::atomic::Ordering::Relaxed))
    });

    hotkey::init().expect("signal handler setup");

    // Integration probes are diagnostic only; they run off the boot path
    // so a slow API doesn't delay the overlay appearing.
    std::thread::spawn(integrations::health::check_and_print);

    std::thread::spawn(move || orchestrator::run_loop(mic, stt, claude, cartesia));

    // Cursor event loop holds the main thread for the rest of the process.
    // Required because winit/Hyprland event loops are main-thread-only.
    ai_cursor::cursor(300, 300);
}
