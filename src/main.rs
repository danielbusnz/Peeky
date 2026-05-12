mod audio;
mod cursor;
mod hotkey;
mod mouse;
mod painter;
mod providers;
mod screenshot;
mod voice;

fn main() {
    let stt =
        providers::stt_deepgram::SttDeepgram::from_env().expect("missing DEEPGRAM_API_KEY");
    let claude = providers::claude::Claude::from_env().expect("missing ANTHROPIC_API_KEY");
    let cartesia =
        providers::tts_cartesia::TtsCartesia::from_env().expect("missing CARTESIA_API_KEY");

    hotkey::init().expect("signal handler setup");
    mouse::spawn_poller();

    std::thread::spawn(move || voice::run_loop(stt, claude, cartesia));

    cursor::cursor(300, 300);
}
