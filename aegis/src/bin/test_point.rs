#[path = "../ai_cursor/mod.rs"]
mod ai_cursor;
#[path = "../mouse_position/mod.rs"]
mod mouse_position;
#[path = "../painter.rs"]
mod painter;
#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../hotkey/mod.rs"]
mod hotkey;
#[path = "../providers/mod.rs"]
mod providers;

use std::time::Duration;

fn main() {
    // Catch SIGUSR1/SIGUSR2 from Hyprland so they don't kill the process.
    hotkey::init().expect("signal handler setup");

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));

        let claude = providers::claude::Claude::from_env(reqwest::Client::new())
            .expect("missing ANTHROPIC_API_KEY");

        let (mx, my, mw, mh) =
            screenshot::active_workspace_geometry().expect("could not query monitor");
        println!("active monitor: {}x{} at ({}, {})", mw, mh, mx, my);

        let (b64, _, _) =
            screenshot::capture_for_claude(mx, my, mw as i32, mh as i32).expect("capture failed");

        let prompt = "Find the most prominent UI element on screen and click it.";
        println!("\nasking Claude (find_action only)...");

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let start = std::time::Instant::now();
        let action = rt
            .block_on(async {
                claude
                    .find_action(
                        prompt,
                        &b64,
                        mx as i64,
                        my as i64,
                        mw as i64,
                        mh as i64,
                        |action| {
                            use providers::claude::Action;
                            if let Action::Click { x, y } = action {
                                eprintln!(
                                    "[cursor] firing at ({}, {}) at +{:?}",
                                    x,
                                    y,
                                    start.elapsed()
                                );
                                ai_cursor::point_at(x as i32, y as i32);
                            }
                        },
                    )
                    .await
            })
            .expect("find_action failed");

        println!("[done] total: {:?}, action: {:?}", start.elapsed(), action);
    });

    ai_cursor::cursor(500, 500);
}
