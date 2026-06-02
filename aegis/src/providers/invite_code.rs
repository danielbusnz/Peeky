//! Optional per-install invite code. When present, the client adds it to
//! proxy requests as `x-aegis-invite-code`, promoting them from the trial
//! tier to the demo tier (see proxy/README.md).
//!
//! Written by the console's onboarding flow to:
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
    load_from_path(&path)
}

/// Testability helper: same logic as `load()` but accepts an explicit path
/// instead of deriving it from the OS config dir. This is the only non-test
/// code change introduced for testing; behavior is identical.
fn load_from_path(path: &PathBuf) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aegis-invite-test-{}-{}.txt",
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
        std::fs::write(&p, "   \n\t  \n").unwrap();
        assert!(load_from_path(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn valid_code_is_trimmed_and_returned() {
        let p = tmp_path("valid");
        std::fs::write(&p, "  MY-INVITE-CODE\n").unwrap();
        assert_eq!(load_from_path(&p), Some("MY-INVITE-CODE".to_string()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn code_with_internal_spaces_is_preserved() {
        // Trim removes leading/trailing whitespace, not interior spaces.
        let p = tmp_path("internal");
        std::fs::write(&p, "\nabc def\n").unwrap();
        assert_eq!(load_from_path(&p), Some("abc def".to_string()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn code_written_without_newline() {
        let p = tmp_path("nonl");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(f, "BARE-CODE").unwrap();
        assert_eq!(load_from_path(&p), Some("BARE-CODE".to_string()));
        let _ = std::fs::remove_file(&p);
    }
}
