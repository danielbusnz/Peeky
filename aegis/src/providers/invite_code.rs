//! Optional per-install invite code. When present, the client adds it to
//! proxy requests as `x-aegis-invite-code`, promoting them from the trial
//! tier to the demo tier (see proxy/README.md).
//!
//! Written by the launcher's onboarding flow to:
//!   Linux:   $XDG_CONFIG_HOME/aegis/invite_code  (or ~/.config/aegis/invite_code)
//!   Windows: %APPDATA%\aegis\invite_code
//!   macOS:   ~/Library/Application Support/aegis/invite_code
//!
//! Absent or empty file means trial tier. Format is not validated here;
//! the proxy is the source of truth and returns 400/403 for bad codes.

use std::fs;
use std::path::PathBuf;

/// Returns the stored invite code, or None if no code is configured.
pub fn load() -> Option<String> {
    let path = invite_code_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn invite_code_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("aegis").join("invite_code"))
}
