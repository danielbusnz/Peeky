mod cursor;
mod movement;
use std::thread;
use std::time::Duration;

fn main() {
    thread::spawn(|| {
        loop {
            let pos = match movement::mouse_movement() {
                Ok(p) => p,
                Err(e) => {
                    println!("Mouse Movement error: {:?}", e);
                    continue;
                }
            };

            println!("mouse at: {:?}", pos);

            match movement::capture_window_at_coords(pos.0 as i16, pos.1 as i16) {
                Some(name) => println!("over window: {}", name),
                None => println!("no window under cursor"),
            }

            match movement::active_window() {
                Some(name) => println!("active window: {}", name),
                None => println!("no active window"),
            }
            thread::sleep(Duration::from_millis(1000));
        }
    });
    cursor::cursor(300, 300);
}
