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

        println!("capturing screenshot...");
        let (b64, w, h) = screenshot::capture_active_workspace()
            .expect("screenshot failed");
        println!("captured {}x{}", w, h);

        let prompt = "Look at the screen. Point at the most prominent UI element \
                      you can see (an icon, button, or window control).";
        println!("\nasking claude (tool use)...");
        let (text, point) = claude
            .ask_with_image_tool(prompt, &b64)
            .expect("claude failed");
        println!("claude: {}\n", text);

        match point {
            Some((x, y)) => {
                let clamped_x = x.clamp(0, w as i32 - 1);
                let clamped_y = y.clamp(0, h as i32 - 1);
                if (clamped_x, clamped_y) != (x, y) {
                    println!(
                        "claude returned out-of-bounds ({}, {}) — clamped to ({}, {})",
                        x, y, clamped_x, clamped_y
                    );
                } else {
                    println!("tool fired with point: ({}, {})", x, y);
                }
                cursor::point_at(clamped_x, clamped_y);
            }
            None => {
                println!("claude didn't invoke the point_at tool");
            }
        }
    });

    cursor::cursor(500, 500);
}
