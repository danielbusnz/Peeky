#![allow(dead_code)]

// Isolated test for the hotkey signal pipeline. Verifies that:
//   1. Hyprland's keybind is sending SIGUSR1/SIGUSR2 to the running peeky pid
//   2. signal_hook's listener thread is catching them
//   3. is_recording() flips on press and off on release
//   4. wait_for_press() unblocks within ~20ms of the press
//
// Run with `cargo run --bin demo_hotkey`. Hold SUPER+space then release.
// Each turn prints when the press was detected and how long you held the key.
// Ctrl+C to quit.

use peeky::hotkey;

use std::time::Instant;

fn main() {
    hotkey::init().expect("signal handler setup");

    println!("hotkey test — hold SUPER+space and release. Ctrl+C to quit.");
    println!();

    let mut turn = 1;
    loop {
        hotkey::wait_for_press();
        let press_t = Instant::now();
        eprintln!("[turn {}] press detected", turn);

        while hotkey::is_recording() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        eprintln!(
            "[turn {}] released after holding {:?}",
            turn,
            press_t.elapsed()
        );
        eprintln!();
        turn += 1;
    }
}
