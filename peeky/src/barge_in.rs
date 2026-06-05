//! Barge-in detector: a background watchdog that flips a CancellationToken
//! when the user presses the hotkey again mid-turn. The orchestrator races
//! its in-flight HTTP streams (Claude, Cartesia) against the token so a new
//! press aborts everything cleanly and the next loop iteration starts fresh.

use crate::hotkey;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Holds a `CancellationToken` that fires when the user presses the hotkey
/// during the current voice turn. Share clones to all tasks that should
/// abort on barge-in via `.cancelled()`.
pub struct BargeIn {
    cancel: CancellationToken,
}

impl BargeIn {
    /// Spawns the watchdog thread. Construct AFTER the hotkey has been
    /// released. Otherwise the watchdog sees the still-pressed state and
    /// fires immediately. Fires once, then the thread exits.
    pub fn start() -> Self {
        let cancel = CancellationToken::new();
        let cancel_w = cancel.clone();
        std::thread::spawn(move || {
            while !cancel_w.is_cancelled() {
                if hotkey::is_recording() {
                    cancel_w.cancel();
                    return;
                }
                // 1ms keeps barge-in latency below user-perceptual floor
                // without burning a measurable CPU slice.
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        BargeIn { cancel }
    }

    /// Owned clone, suitable to `move` into a spawned task.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for BargeIn {
    /// Cancels the token to signal the watchdog thread to exit. Safe
    /// because all task observers should have finished by the time
    /// BargeIn drops at the end of a voice turn.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
