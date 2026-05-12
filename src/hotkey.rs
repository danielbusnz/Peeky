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
                SIGUSR1 => RECORDING.store(true, Ordering::Relaxed),
                SIGUSR2 => RECORDING.store(false, Ordering::Relaxed),
                _ => {}
            }
        }
    });
    Ok(())
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}
