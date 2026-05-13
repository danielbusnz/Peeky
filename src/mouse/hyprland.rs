use hyprland::data::CursorPosition;
use hyprland::shared::HyprData;
use std::thread;
use std::time::Duration;

pub fn mouse_movement() -> hyprland::Result<(i64, i64)> {
    let pos = CursorPosition::get()?;
    Ok((pos.x, pos.y))
}

/// Spawns a background thread that polls the cursor position every second.
pub fn spawn_poller() {
    thread::spawn(|| {
        loop {
            match mouse_movement() {
                Ok(pos) => println!("mouse at: {:?}", pos),
                Err(e) => println!("Mouse Movement error: {:?}", e),
            }
            thread::sleep(Duration::from_millis(1000));
        }
    });
}
