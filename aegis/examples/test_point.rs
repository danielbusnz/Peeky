#![allow(dead_code)]

#[path = "../src/ai_cursor/mod.rs"]
mod ai_cursor;
#[path = "../src/mouse_position/mod.rs"]
mod mouse_position;
#[path = "../src/painter.rs"]
mod painter;
#[path = "../src/screenshot/mod.rs"]
mod screenshot;
#[path = "../src/hotkey/mod.rs"]
mod hotkey;
#[path = "../src/tuning.rs"]
mod tuning;
#[path = "../src/providers/mod.rs"]
mod providers;

use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn main() {
    hotkey::init().expect("signal handler setup");

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));

        let claude = providers::claude::Claude::from_env(reqwest::Client::new())
            .expect("missing ANTHROPIC_API_KEY");

        let (mx, my, mw, mh) =
            screenshot::active_workspace_geometry().expect("could not query monitor");
        println!("active monitor: {}x{} at ({}, {})", mw, mh, mx, my);

        let (dw, dh) = screenshot::pick_declared_resolution(mw as i64, mh as i64);
        let initial_b64 =
            screenshot::capture_resized_for_claude(mx, my, mw as i32, mh as i32, dw, dh)
                .expect("capture failed");

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let start = std::time::Instant::now();
        let result = rt.block_on(async {
            let take_screenshot =
                move || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                    let (cx, cy, cw, ch) = screenshot::active_workspace_geometry()
                        .map(|g| (g.0, g.1, g.2 as i32, g.3 as i32))
                        .unwrap_or((mx, my, mw as i32, mh as i32));
                    let (dw, dh) = screenshot::pick_declared_resolution(cw as i64, ch as i64);
                    screenshot::capture_resized_for_claude(cx, cy, cw, ch, dw, dh).map_err(
                        |e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() },
                    )
                };
            claude
                .run_agent_loop(
                    "Find the most prominent UI element on screen and click it.",
                    &initial_b64,
                    &[],
                    None,
                    mx as i64,
                    my as i64,
                    mw as i64,
                    mh as i64,
                    vec![],
                    CancellationToken::new(),
                    take_screenshot,
                    |action| {
                        use providers::claude::Action;
                        eprintln!("[action +{:?}] {:?}", start.elapsed(), action);
                        match action {
                            Action::Point { x, y } | Action::Click { x, y } => {
                                ai_cursor::point_at(x as i32, y as i32);
                            }
                            _ => {}
                        }
                    },
                    |_name, _input| None,
                    |_token| {},
                )
                .await
        });

        match result {
            Ok(text) => println!(
                "[done] total: {:?}, final text: {:?}",
                start.elapsed(),
                text
            ),
            Err(e) => eprintln!("[error] {}", e),
        }
    });

    ai_cursor::cursor(500, 500);
}
