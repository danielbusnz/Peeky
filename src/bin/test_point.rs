#[path = "../cursor.rs"]
mod cursor;
#[path = "../mouse.rs"]
mod mouse;
#[path = "../painter.rs"]
mod painter;
#[path = "../screenshot.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use std::time::Duration;

fn main() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));

        let claude = providers::claude::Claude::from_env()
            .expect("missing ANTHROPIC_API_KEY");

        let (mx, my, mw, mh) =
            screenshot::active_workspace_geometry().expect("could not query monitor");
        println!("active monitor: {}x{} at ({}, {})", mw, mh, mx, my);

        let prompt = "Look at the screen. Point at the most prominent UI element \
                      you can see (an icon, button, or window control).";
        println!("\nasking claude (Computer Use + Haiku)...");
        let (text, point) = claude
            .detect_element_location(prompt, mx as i64, my as i64, mw as i64, mh as i64)
            .expect("claude failed");
        println!("claude: {}\n", text);

        match point {
            Some((px, py)) => {
                println!("tool fired with point: ({}, {})", px, py);
                cursor::point_at(px as i32, py as i32);
            }
            None => {
                println!("claude didn't invoke the computer tool");
            }
        }
    });

    cursor::cursor(500, 500);
}
