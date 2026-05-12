mod audio;
mod cursor;
mod hotkey;
mod mouse;
mod painter;
mod providers;
mod screenshot;

fn main() {
    let claude = providers::claude::Claude::from_env().expect("missing ANTHROPIC_API_KEY");
    use providers::Llm;
    match claude.complete("Say hi in 5 words.") {
        Ok(text) => println!("Claude: {}", text),
        Err(e) => eprintln!("Claude error: {}", e),
    }

    hotkey::init().expect("signal handler setup");

    mouse::spawn_poller();

    cursor::cursor(300, 300);
}
