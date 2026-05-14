// End-to-end smoke test for the cross-platform pieces.
//
// Exercises: hotkey + mouse + screenshot + cursor (incl. click-through)
//
// Run with:
//   Hyprland:  cargo run --bin test_win
//   Windows:   cargo run --bin test_win --no-default-features \
//                  --features winit-window,crossplatform
//
// What it does:
//   - Initializes the hotkey listener (Insert key)
//   - Boots the cursor overlay on the main thread
//   - Background thread waits for each Insert press, then:
//        1. queries mouse position
//        2. captures a screenshot, saves to a temp file
//        3. flies the cursor sprite to the mouse position
//        4. waits for release
//
// Visual check on Windows: while holding Insert, the cursor sprite should
// be visible AND clicks should pass through to the app underneath.

#[path = "../cursor/mod.rs"]
mod cursor;
#[path = "../hotkey/mod.rs"]
mod hotkey;
#[path = "../mouse/mod.rs"]
mod mouse;
#[path = "../screenshot/mod.rs"]
mod screenshot;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::time::Instant;

fn main() {
    println!("=================================");
    println!("aegis end-to-end test");
    println!("=================================");
    println!();

    hotkey::init().expect("hotkey init failed");
    println!("[init] hotkey listener up");
    println!("[init] hold INSERT to trigger a turn, release to stop");
    println!("[init] Ctrl+C to quit");
    println!();

    std::thread::spawn(|| {
        let mut turn = 1;
        loop {
            hotkey::wait_for_press();
            let t = Instant::now();
            eprintln!("──── turn {} ──────────────────────────", turn);
            log_t(&t, "press detected");

            // 1. Mouse position
            match mouse::mouse_movement() {
                Ok((x, y)) => eprintln!(
                    "[t={:>8.1?}] mouse at ({}, {})",
                    t.elapsed(),
                    x,
                    y
                ),
                Err(e) => eprintln!(
                    "[t={:>8.1?}] mouse query failed: {}",
                    t.elapsed(),
                    e
                ),
            }

            // 2. Screenshot
            match screenshot::active_workspace_geometry() {
                Ok((mx, my, mw, mh)) => {
                    eprintln!(
                        "[t={:>8.1?}] monitor: {}x{} at ({}, {})",
                        t.elapsed(),
                        mw,
                        mh,
                        mx,
                        my
                    );
                    match screenshot::capture_for_claude(mx, my, mw as i32, mh as i32) {
                        Ok((b64, w, h)) => {
                            eprintln!(
                                "[t={:>8.1?}] captured {}x{} ({} base64 bytes)",
                                t.elapsed(),
                                w,
                                h,
                                b64.len()
                            );
                            let bytes = BASE64.decode(b64.as_bytes()).expect("base64 decode");
                            let path = std::env::temp_dir()
                                .join(format!("aegis_test_turn{}.jpg", turn));
                            std::fs::write(&path, &bytes).expect("write screenshot");
                            eprintln!(
                                "[t={:>8.1?}] saved: {}",
                                t.elapsed(),
                                path.display()
                            );
                        }
                        Err(e) => eprintln!(
                            "[t={:>8.1?}] capture failed: {}",
                            t.elapsed(),
                            e
                        ),
                    }
                }
                Err(e) => eprintln!(
                    "[t={:>8.1?}] monitor query failed: {}",
                    t.elapsed(),
                    e
                ),
            }

            // 3. Fly cursor to mouse position
            if let Ok((mx, my)) = mouse::mouse_movement() {
                cursor::point_at(mx as i32, my as i32);
                eprintln!(
                    "[t={:>8.1?}] cursor::point_at({}, {}) fired",
                    t.elapsed(),
                    mx,
                    my
                );
            }

            // 4. Wait for release
            while hotkey::is_recording() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            log_t(&t, "released");
            eprintln!();
            turn += 1;
        }
    });

    // Main thread owns the cursor window forever.
    cursor::cursor(500, 500);
}

fn log_t(t: &Instant, msg: &str) {
    eprintln!("[t={:>8.1?}] {}", t.elapsed(), msg);
}
