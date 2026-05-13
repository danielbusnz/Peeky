use signal_hook::consts::{SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static RECORDING: AtomicBool = AtomicBool::new(false);

pub fn init() -> std::io::Result<()> {
    let mut signals = Signals::new([SIGUSR1, SIGUSR2])?;
    thread::spawn(move || {
        for sig in &mut signals {
            match sig {
                SIGUSR1 => {
                    eprintln!("[hotkey] SIGUSR1 received (press)");
                    RECORDING.store(true, Ordering::Relaxed);
                }
                SIGUSR2 => {
                    eprintln!("[hotkey] SIGUSR2 received (release)");
                    RECORDING.store(false, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    });
    Ok(())
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

pub fn wait_for_press() {
    while !is_recording() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
