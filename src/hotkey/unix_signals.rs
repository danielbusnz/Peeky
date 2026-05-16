use signal_hook::consts::{SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static RECORDING: AtomicBool = AtomicBool::new(false);

// Optional callbacks so the cursor overlay can react to signal events without
// creating a circular crate:: dependency visible to test binaries.
static ON_PRESS: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_RELEASE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Register callbacks invoked immediately after SIGUSR1 (press) and SIGUSR2
/// (release). Must be called before `init`. Each OnceLock accepts at most one
/// registration; subsequent calls are silently ignored.
pub fn on_press(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_PRESS.set(Box::new(f));
}
pub fn on_release(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_RELEASE.set(Box::new(f));
}

pub fn init() -> std::io::Result<()> {
    let mut signals = Signals::new([SIGUSR1, SIGUSR2])?;
    thread::spawn(move || {
        for sig in &mut signals {
            match sig {
                SIGUSR1 => {
                    eprintln!("[hotkey] SIGUSR1 received (press)");
                    RECORDING.store(true, Ordering::Relaxed);
                    if let Some(f) = ON_PRESS.get() {
                        f();
                    }
                }
                SIGUSR2 => {
                    eprintln!("[hotkey] SIGUSR2 received (release)");
                    RECORDING.store(false, Ordering::Relaxed);
                    if let Some(f) = ON_RELEASE.get() {
                        f();
                    }
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
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
