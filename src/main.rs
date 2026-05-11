mod cursor;
mod mouse;
mod screenshot;
mod windows;
use std::thread;
use std::time::Duration;

fn main() {
    screenshot::take_area_screenshot(0, 0, 800, 600, "/tmp/aegis-test.png");

    thread::spawn(|| {
        loop {
            let pos = match mouse::mouse_movement() {
                Ok(p) => p,
                Err(e) => {
                    println!("Mouse Movement error: {:?}", e);
                    continue;
                }
            };

            println!("mouse at: {:?}", pos);

            match windows::capture_window_at_coords(pos.0 as i16, pos.1 as i16) {
                Some(name) => println!("over window: {}", name),
                None => println!("no window under cursor"),
            }

            match windows::active_window() {
                Some(name) => println!("active window: {}", name),
                None => println!("no active window"),
            }
            thread::sleep(Duration::from_millis(1000));
        }
    });
    cursor::cursor(300, 300);
}
