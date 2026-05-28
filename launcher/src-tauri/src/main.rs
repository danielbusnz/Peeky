#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Wire constants for the Cloudflare Worker proxy. The launcher does not depend
// on the aegis crate, so the headers are duplicated here and code_format_valid
// mirrors CODE_RE from proxy/src/index.ts. If the wire values ever change,
// update proxy/src/index.ts first, then this module and
// aegis/src/providers/proxy_contract.rs together.
mod proxy_contract {
    pub const DEVICE_ID_HEADER: &str = "x-aegis-device-id";
    pub const INVITE_CODE_HEADER: &str = "x-aegis-invite-code";

    /// Mirrors the proxy's CODE_RE: /^[A-Z0-9][A-Z0-9-]{6,62}[A-Z0-9]$/
    pub fn code_format_valid(s: &str) -> bool {
        let bytes = s.as_bytes();
        if !(8..=64).contains(&bytes.len()) {
            return false;
        }
        let all_valid = bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'-');
        if !all_valid {
            return false;
        }
        bytes.first() != Some(&b'-') && bytes.last() != Some(&b'-')
    }
}

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

    // 1. Sibling of the launcher exe (the shipped-bundle sidecar).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("aegis"));
            #[cfg(windows)]
            candidates.push(dir.join("aegis.exe"));
        }
    }

    // 2/3. Workspace dev layout.
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
            apply_byok_env(&mut cmd);
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

/// OS keychain service the launcher stores the user's own provider keys
/// under. Each provider is a separate account.
const KEYRING_SERVICE: &str = "com.aegis.settings";

/// (keychain account, aegis "go direct" flag, provider key env var) per
/// provider. The aegis runtime already switches a provider to direct mode
/// when its DIRECT flag is present, reading the key from the matching var.
const BYOK_PROVIDERS: [(&str, &str, &str); 3] = [
    ("anthropic", "AEGIS_ANTHROPIC_DIRECT", "ANTHROPIC_API_KEY"),
    ("deepgram", "AEGIS_DEEPGRAM_DIRECT", "DEEPGRAM_API_KEY"),
    ("cartesia", "AEGIS_CARTESIA_DIRECT", "CARTESIA_API_KEY"),
];

fn keychain_get(account: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).ok()?;
    match entry.get_password() {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => None,
    }
}

/// Inject any stored BYOK keys into the aegis child using the env contract
/// its providers already understand. Providers with no stored key are left
/// on the proxy/trial path, so direct + trial can mix per provider.
fn apply_byok_env(cmd: &mut Command) {
    for (account, direct_flag, key_var) in BYOK_PROVIDERS {
        if let Some(key) = keychain_get(account) {
            cmd.env(direct_flag, "1").env(key_var, key);
        }
    }
}

/// Persist the user's own provider keys to the OS keychain. A blank value
/// is skipped, not cleared: the UI pre-fills a "saved" hint for providers
/// that already have a key, so submitting the form blank must keep them.
#[tauri::command]
fn save_api_keys(anthropic: String, deepgram: String, cartesia: String) -> Result<(), String> {
    let values = [anthropic, deepgram, cartesia];
    for ((account, _, _), value) in BYOK_PROVIDERS.iter().zip(values.iter()) {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
        entry
            .set_password(trimmed)
            .map_err(|e| format!("save {account}: {e}"))?;
    }
    Ok(())
}

/// Which providers currently have a stored key. Booleans only, never the
/// secret values, so the UI can show "saved" state without exposing keys.
#[tauri::command]
fn api_keys_status() -> std::collections::HashMap<String, bool> {
    BYOK_PROVIDERS
        .iter()
        .map(|(account, _, _)| (account.to_string(), keychain_get(account).is_some()))
        .collect()
}

/// Local format check. Mirrors the proxy's CODE_RE so we fail on obvious
/// junk before the proxy ever sees it. The proxy is the source of truth
/// for expiry, device limits, and unknown codes.
fn validate_code(code: &str) -> Result<(), &'static str> {
    if proxy_contract::code_format_valid(code) {
        Ok(())
    } else {
        Err("invalid invite code format")
    }
}

/// Proxy endpoint that read-only-validates an invite code (no device binding,
/// no usage charged). See proxy/src/index.ts handleInviteVerify.
const VERIFY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/invite/verify";

/// Returns this install's device id, creating + persisting one if absent.
/// Mirrors aegis/src/providers/device_id.rs so the launcher and the agent
/// agree on the same id at `~/.config/aegis/device_id`.
fn device_id() -> Result<String, String> {
    let path = dirs::config_dir()
        .ok_or("no config dir on this platform")?
        .join("aegis")
        .join("device_id");

    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if uuid::Uuid::parse_str(trimmed).is_ok() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, &new_id).map_err(|e| format!("write device_id: {e}"))?;
    Ok(new_id)
}

/// Pre-flight check the onboarding UI runs when the user presses Enter on the
/// invite field. Format-checks locally for instant feedback, then asks the
/// proxy whether the code is real, unexpired, and has a device slot. `Ok`
/// means usable (green); `Err(message)` carries a reason to show (red).
#[tauri::command]
async fn verify_invite_code(code: String) -> Result<(), String> {
    let trimmed = code.trim();
    validate_code(trimmed).map_err(str::to_string)?;

    let device_id = device_id()?;
    let resp = reqwest::Client::new()
        .post(VERIFY_URL)
        .header(proxy_contract::DEVICE_ID_HEADER, device_id)
        .header(proxy_contract::INVITE_CODE_HEADER, trimmed)
        .send()
        .await
        .map_err(|_| "Couldn't reach the server. Check your connection.".to_string())?;

    if resp.status().is_success() {
        return Ok(());
    }

    // Surface the proxy's human message when present, else its error code.
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    let reason = body
        .get("message")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("invalid invite code")
        .to_string();
    Err(reason)
}

/// Live-validate the user's own provider keys by hitting each provider's
/// auth the same way aegis does in direct mode. Returns a per-provider map
/// of whether the key works. An empty key is reported `false`. Used by the
/// onboarding gate so a typo'd key can't slip through.
#[tauri::command]
async fn verify_api_keys(
    anthropic: String,
    deepgram: String,
    cartesia: String,
) -> std::collections::HashMap<String, bool> {
    // A blank field falls back to the key already in the keychain, so a
    // returning user who left it blank ("leave blank to keep") is still
    // checked against the key that will actually be used.
    let resolve = |passed: String, account: &str| -> String {
        let t = passed.trim();
        if t.is_empty() {
            keychain_get(account).unwrap_or_default()
        } else {
            t.to_string()
        }
    };
    let anthropic = resolve(anthropic, "anthropic");
    let deepgram = resolve(deepgram, "deepgram");
    let cartesia = resolve(cartesia, "cartesia");

    let client = reqwest::Client::new();
    let mut out = std::collections::HashMap::new();
    out.insert(
        "anthropic".to_string(),
        check_anthropic(&client, &anthropic).await,
    );
    out.insert(
        "deepgram".to_string(),
        check_deepgram(&client, &deepgram).await,
    );
    out.insert(
        "cartesia".to_string(),
        check_cartesia(&client, &cartesia).await,
    );
    out
}

async fn check_anthropic(client: &reqwest::Client, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_deepgram(client: &reqwest::Client, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    client
        .post("https://api.deepgram.com/v1/auth/grant")
        .header("authorization", format!("Token {key}"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "ttl_seconds": 60 }))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_cartesia(client: &reqwest::Client, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    client
        .post("https://api.cartesia.ai/access-token")
        .header("authorization", format!("Bearer {key}"))
        .header("cartesia-version", "2026-03-01")
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "grants": { "tts": true }, "expires_in": 60 }))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn is_onboarded() -> bool {
    dirs::config_dir()
        .map(|d| d.join("aegis").join("onboarded").exists())
        .unwrap_or(false)
}

#[tauri::command]
fn mark_onboarded() -> Result<(), String> {
    let dir = dirs::config_dir().ok_or("no config dir")?.join("aegis");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    std::fs::write(dir.join("onboarded"), "").map_err(|e| format!("{e}"))?;
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
    // If already onboarded, spawn aegis directly and exit (no UI).
    if is_onboarded() {
        if let Err(e) = spawn_aegis() {
            eprintln!("[launcher] {e}");
        }
        return;
    }

    // webkit2gtk's DMABUF renderer crashes against Hyprland and several
    // other Wayland compositors with "Error 71 (Protocol error)". Disabling
    // it forces a software path that works everywhere. Harmless on non-Linux
    // platforms but gated since the env var only exists on Linux.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let builder = tauri::Builder::default().invoke_handler(tauri::generate_handler![
        spawn_aegis,
        save_invite_code,
        mark_onboarded,
        verify_invite_code,
        save_api_keys,
        api_keys_status,
        verify_api_keys
    ]);

    // macOS only: the permission plugin lets onboarding prompt for mic, screen
    // recording, and accessibility before the agent spawns. Compiled out
    // elsewhere, so the Linux/Windows builds are unchanged.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_macos_permissions::init());

    builder
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
