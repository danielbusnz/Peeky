//! Barge-in detector: a background watchdog that flips a CancellationToken
//! when the user presses the hotkey again mid-turn. The orchestrator races
//! its in-flight HTTP streams (Claude, Cartesia) against the token so a new
//! press aborts everything cleanly and the next loop iteration starts fresh.

use crate::hotkey;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Spawns a background thread that watches for the user pressing the hotkey
/// AGAIN (after the current turn's release), and cancels a shared
/// CancellationToken when it happens. Async code can race against the
/// token's `.cancelled()` future to abort their in-flight work.
///
/// On drop, cancels the token to signal the watchdog thread to exit. By
/// the time BargeIn drops, all tasks observing the token have completed,
/// so the cleanup cancel doesn't affect them. Construct AFTER the hotkey
/// has been released (RECORDING is false); the watchdog interprets the
/// next true→false→true cycle as a new press.
pub struct BargeIn {
    cancel: CancellationToken,
}

impl BargeIn {
    pub fn start() -> Self {
        let cancel = CancellationToken::new();
        let cancel_w = cancel.clone();
        std::thread::spawn(move || {
            while !cancel_w.is_cancelled() {
                if hotkey::is_recording() {
                    cancel_w.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        BargeIn { cancel }
    }

    /// Owned clone of the cancellation token. Each spawned task takes one
    /// and awaits `.cancelled()` to learn about barge-in.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for BargeIn {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
