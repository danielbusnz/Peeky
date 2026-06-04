//! Shared helper for macOS integrations that drive scriptable apps via
//! `osascript`. It exposes no tools of its own; other integration modules call
//! `run` when their AppleScript backend is the one selected for this machine.
//!
//! Only meaningful on macOS. Callers gate on `cfg!(target_os = "macos")` before
//! reaching here, so on other platforms `run` is never invoked. The code still
//! compiles everywhere: it is just `std::process::Command`.

use std::process::Command;

/// Run an AppleScript snippet via `osascript -e` and return trimmed stdout.
///
/// `Err` carries osascript's stderr (e.g. "application isn't running") so the
/// caller can surface a meaningful message to Claude instead of failing
/// silently. Fire-and-forget callers can ignore the `Ok` payload.
pub fn run(script: &str) -> Result<String, String> {
    let output = Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| format!("osascript spawn failed: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
