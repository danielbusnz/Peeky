use aegis::{
    actions, ai_cursor, audio, hotkey, integrations, orchestrator, painter, providers, routelet,
};
// Only used by the macOS screen-recording permission trigger below.
#[cfg(target_os = "macos")]
use aegis::screenshot;

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

    // Load the local ONNX classifier. Asset path: AEGIS_ROUTELET_DIR env var,
    // or the default models/routelet relative to the working directory.
    let routelet_dir = std::env::var("AEGIS_ROUTELET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("models/routelet"));
    let routelet = routelet::Routelet::load(&routelet_dir).unwrap_or_else(|e| {
        panic!(
            "routelet: failed to load from {}: {e}",
            routelet_dir.display()
        )
    });
    eprintln!(
        "[startup] routelet classifier loaded from {}",
        routelet_dir.display()
    );

    actions::init_input_executor();
    actions::check_input_injection_available();

    // Trigger screen recording and microphone permission prompts on macOS.
    // This runs early so the permission dialogs don't interrupt the voice flow.
    #[cfg(target_os = "macos")]
    {
        let _ = screenshot::capture_for_claude(0, 0, 100, 100);
        eprintln!("[startup] screen recording permission check triggered");
        audio::trigger_mic_permission();
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

    std::thread::spawn(move || orchestrator::run_loop(mic, stt, claude, cartesia, routelet));

    // Cursor event loop holds the main thread for the rest of the process.
    // Required because winit/Hyprland event loops are main-thread-only.
    ai_cursor::cursor(300, 300);
}
