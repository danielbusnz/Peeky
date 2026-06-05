//! Optional signed-in session token. When present, the client sends it on
//! proxy requests as `Authorization: Bearer <jwt>`, promoting them from the
//! trial tier to the account tier (see proxy/src/tiers.ts).
//!
//! Written by the console after GitHub sign-in (it also keeps the token in the
//! OS keychain; this file is the copy the already-running agent reads) to:
//!   Linux:   $XDG_CONFIG_HOME/peeky/session_jwt  (or ~/.config/peeky/session_jwt)
//!   Windows: %APPDATA%\peeky\session_jwt
//!   macOS:   ~/Library/Application Support/peeky/session_jwt
//!
//! Absent or empty file means no session (trial tier). Re-read per request so a
//! sign-in mid-session takes effect on the next turn without restarting peeky.
//! Signature and expiry are not checked here; the proxy is the source of truth
//! and silently falls back to the trial tier for a bad or expired token.

use std::fs;
use std::path::PathBuf;

/// Returns the stored session JWT, or None if the user isn't signed in.
pub fn load() -> Option<String> {
    let path = session_jwt_path()?;
    load_from_path(&path)
}

/// Write the token to the file `load()` reads (0600 on Unix). Used by the
/// agent-driven sign-in flow so a successful login takes effect on the next
/// turn. The console writes the same file after its own sign-in.
pub fn store(token: &str) -> std::io::Result<()> {
    let path = session_jwt_path()
        .ok_or_else(|| std::io::Error::other("no config dir on this platform"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Same logic as `load()` but against an explicit path, for tests.
fn load_from_path(path: &PathBuf) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn session_jwt_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("peeky").join("session_jwt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "peeky-session-jwt-test-{}-{}.txt",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_returns_none() {
        let p = tmp_path("missing");
        let _ = std::fs::remove_file(&p);
        assert!(load_from_path(&p).is_none());
    }

    #[test]
    fn empty_file_returns_none() {
        let p = tmp_path("empty");
        std::fs::write(&p, "").unwrap();
        assert!(load_from_path(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn whitespace_only_returns_none() {
        let p = tmp_path("whitespace");
        std::fs::write(&p, "  \n\t \n").unwrap();
        assert!(load_from_path(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn token_is_trimmed_and_returned() {
        let p = tmp_path("valid");
        std::fs::write(&p, "  ey.some.jwt\n").unwrap();
        assert_eq!(load_from_path(&p), Some("ey.some.jwt".to_string()));
        let _ = std::fs::remove_file(&p);
    }
}
