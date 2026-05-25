#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Launch the actual aegis cursor + voice agent as a child process.
///
/// Path lookup order:
/// 1. Sibling of the launcher executable. In a shipped `.app`/`.msi`
///    bundle, Tauri's `externalBin` config drops the aegis binary next
///    to the launcher in `Contents/MacOS/` (macOS) or alongside the
///    launcher exe (Windows/Linux). This is the production path.
/// 2. `../../target/{debug,release}/aegis`: workspace dev layout, used
///    by `cargo tauri dev` where the launcher's cwd is
///    `launcher/src-tauri/`.
/// 3. `target/{debug,release}/aegis`: workspace root cwd, used if the
///    launcher is launched directly from the project root.
#[tauri::command]
fn spawn_aegis() -> Result<(), String> {
    use std::path::PathBuf;

    let mut candidates: Vec<PathBuf> = Vec::new();

    // Sibling-of-launcher: works for shipped bundles where Tauri's
    // externalBin places the aegis sidecar in the same dir as the
    // launcher's own executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("aegis"));
            #[cfg(windows)]
            candidates.push(dir.join("aegis.exe"));
        }
    }

    // Dev paths: when running via cargo tauri dev or directly from
    // the workspace.
    candidates.extend(
        [
            "../../target/debug/aegis",
            "../../target/release/aegis",
            "target/debug/aegis",
            "target/release/aegis",
        ]
        .iter()
        .map(PathBuf::from),
    );

    for path in &candidates {
        if path.exists() {
            eprintln!("[launcher] spawning aegis from: {}", path.display());
            let mut cmd = Command::new(path);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            #[cfg(unix)]
            cmd.process_group(0);
            if let Ok(_child) = cmd.spawn() {
                return Ok(());
            }
        }
    }

    let tried = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "aegis binary not found. Tried: {tried}. Build it with \
         `cargo build --release -p aegis --no-default-features --features winit-window,crossplatform` first."
    ))
}

/// Local format check. Mirrors the proxy's CODE_RE so we fail on obvious
/// junk before the proxy ever sees it. The proxy is the source of truth
/// for expiry, device limits, and unknown codes.
fn validate_code(code: &str) -> Result<(), &'static str> {
    let bytes = code.as_bytes();
    if !(8..=64).contains(&bytes.len()) {
        return Err("invalid invite code format");
    }
    let valid_chars = bytes
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'-');
    if !valid_chars {
        return Err("invalid invite code format");
    }
    if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') {
        return Err("invalid invite code format");
    }
    Ok(())
}

/// Validate and persist an invite code to the same config dir aegis reads
/// from at startup (see aegis/src/providers/invite_code.rs). The empty
/// string clears the code.
#[tauri::command]
fn save_invite_code(code: String) -> Result<(), String> {
    let trimmed = code.trim();
    if !trimmed.is_empty() {
        validate_code(trimmed).map_err(str::to_string)?;
    }

    let dir = dirs::config_dir()
        .ok_or("no config dir on this platform")?
        .join("aegis");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create_dir_all: {e}"))?;
    let path = dir.join("invite_code");
    std::fs::write(&path, trimmed).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

fn main() {
    // webkit2gtk's DMABUF renderer crashes against Hyprland and several
    // other Wayland compositors with "Error 71 (Protocol error)". Disabling
    // it forces a software path that works everywhere. Harmless on non-Linux
    // platforms but gated since the env var only exists on Linux.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![spawn_aegis, save_invite_code])
        .run(tauri::generate_context!())
        .expect("error running launcher");
}

#[cfg(test)]
mod tests {
    use super::validate_code;

    #[test]
    fn accepts_typical_recruiter_code() {
        assert!(validate_code("RECRUITER-ACME-7K2X").is_ok());
    }

    #[test]
    fn accepts_minimum_length() {
        assert!(validate_code("ABCDEFGH").is_ok());
    }

    #[test]
    fn rejects_too_short() {
        assert!(validate_code("ABC").is_err());
    }

    #[test]
    fn rejects_too_long() {
        assert!(validate_code(&"A".repeat(65)).is_err());
    }

    #[test]
    fn rejects_lowercase() {
        assert!(validate_code("recruiter-acme").is_err());
    }

    #[test]
    fn rejects_special_chars() {
        assert!(validate_code("RECRUITER!ACME").is_err());
        assert!(validate_code("RECRUITER ACME").is_err());
    }

    #[test]
    fn rejects_leading_or_trailing_dash() {
        assert!(validate_code("-RECRUITER-ACME").is_err());
        assert!(validate_code("RECRUITER-ACME-").is_err());
    }
}
