//! Single-instance guard.
//!
//! A relaunch (e.g. after the user grants Screen Recording, which only takes
//! effect on restart) spawns a fresh aegis, but the launcher never kills the
//! previous one, so two instances would run at once. On startup we terminate
//! the previously-recorded instance, then record our own pid as the live one.

use std::process::Command;

/// Kill any prior aegis instance, then record this process as the live one.
/// Best effort: any failure just leaves the prior instance running rather than
/// blocking startup. Unix-only (uses `ps`/`kill`); a no-op elsewhere.
#[cfg(unix)]
pub fn enforce() {
    let Some(dir) = dirs::config_dir() else { return };
    let pid_path = dir.join("aegis").join("aegis.pid");
    let me = std::process::id();

    if let Ok(contents) = std::fs::read_to_string(&pid_path) {
        if let Ok(old) = contents.trim().parse::<u32>() {
            // Only signal a live process that is actually an aegis, so a reused
            // pid can't take down something unrelated.
            if old != me && is_aegis(old) {
                let _ = Command::new("kill").arg(old.to_string()).status();
                eprintln!("[singleton] terminated previous aegis (pid {old})");
            }
        }
    }

    if let Some(parent) = pid_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&pid_path, me.to_string());
}

#[cfg(not(unix))]
pub fn enforce() {}

/// True if `pid` is a running process whose command name contains "aegis".
#[cfg(unix)]
fn is_aegis(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains("aegis")
        })
        .unwrap_or(false)
}
