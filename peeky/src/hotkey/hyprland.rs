use signal_hook::consts::{SIGUSR1, SIGUSR2};
use signal_hook::iterator::Signals;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::backend::HotkeyBackend;

/// Shared push-to-talk state. Set by SIGUSR1, cleared by SIGUSR2.
/// Polled by `wait_for_press()` and the orchestrator's mic-forwarder.
static RECORDING: AtomicBool = AtomicBool::new(false);

/// Callbacks the cursor overlay registers without creating a direct
/// `crate::` dependency that would bleed into test binaries.
static ON_PRESS: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_RELEASE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Hyprland signal backend. Hyprland's `bind`/`bindr` send SIGUSR1/SIGUSR2
/// to processes matching its target regex. Zero-sized; never instantiated.
pub struct Backend;

impl HotkeyBackend for Backend {
    /// Starts the signal-listener thread that translates SIGUSR1/SIGUSR2 into
    /// RECORDING transitions. `poll()` uses the default no-op: this listener
    /// runs independently, so there is no queue to drain.
    fn init() -> std::io::Result<()> {
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

    fn is_recording() -> bool {
        RECORDING.load(Ordering::Relaxed)
    }

    /// 1ms poll keeps latency well below human-perceptual without burning a
    /// measurable CPU slice.
    fn wait_for_press() {
        while !Self::is_recording() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Registers a callback fired immediately after SIGUSR1. Call before
    /// `init()`. At most one per process; later registrations are ignored.
    fn on_press(f: Box<dyn Fn() + Send + Sync + 'static>) {
        let _ = ON_PRESS.set(f);
    }

    /// Registers a callback fired immediately after SIGUSR2. Call before
    /// `init()`. At most one per process; later registrations are ignored.
    fn on_release(f: Box<dyn Fn() + Send + Sync + 'static>) {
        let _ = ON_RELEASE.set(f);
    }
}
