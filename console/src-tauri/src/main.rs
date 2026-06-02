#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Wire constants for the Cloudflare Worker proxy. The console does not depend
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

use tauri::Manager;

/// Launch the actual aegis cursor + voice agent as a child process.
///
/// Path lookup order:
/// 1. Sibling of the console executable. In a shipped `.app`/`.msi`
///    bundle, Tauri's `externalBin` config drops the aegis binary next
///    to the console in `Contents/MacOS/` (macOS) or alongside the
///    console exe (Windows/Linux). This is the production path.
/// 2. `../../target/{debug,release}/aegis`: workspace dev layout, used
///    by `cargo tauri dev` where the console's cwd is
///    `console/src-tauri/`.
/// 3. `target/{debug,release}/aegis`: workspace root cwd, used if the
///    console is launched directly from the project root.
/// Candidate paths to the aegis binary, best-first: the shipped-bundle sidecar
/// (sibling of the console exe), then the workspace dev layouts. Shared by
/// spawn_aegis and the integrations-status shell-out.
fn aegis_candidates() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Sibling of the console exe (the shipped-bundle sidecar).
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

    candidates
}

#[tauri::command]
fn spawn_aegis() -> Result<(), String> {
    let candidates = aegis_candidates();
    let routelet_dir = resolve_routelet_dir();

    for path in &candidates {
        if path.exists() {
            eprintln!("[console] spawning aegis from: {}", path.display());
            let mut cmd = Command::new(path);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            apply_byok_env(&mut cmd);
            if let Some(dir) = &routelet_dir {
                eprintln!("[console] AEGIS_ROUTELET_DIR={}", dir.display());
                cmd.env("AEGIS_ROUTELET_DIR", dir);
            }
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

/// Find the routelet ONNX model directory to pass to aegis as
/// `AEGIS_ROUTELET_DIR`. Production path is the bundled Resources dir; dev
/// paths cover `cargo tauri dev` and a workspace-root cwd. If the user has
/// already set `AEGIS_ROUTELET_DIR`, respect it. Returns `None` when no
/// candidate exists so aegis falls back to its own default and fails loud.
fn resolve_routelet_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Ok(existing) = std::env::var("AEGIS_ROUTELET_DIR") {
        let p = PathBuf::from(existing);
        if p.exists() {
            return Some(p);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // macOS .app: Contents/MacOS/<exe> -> Contents/Resources/models/routelet
            candidates.push(dir.join("../Resources/models/routelet"));
            // Linux/Windows bundles: resources next to the exe.
            candidates.push(dir.join("resources/models/routelet"));
            // Dev: target/{debug,release}/<console> -> workspace/models/routelet
            candidates.push(dir.join("../../models/routelet"));
        }
    }

    // `cargo tauri dev` cwd is console/src-tauri/; workspace-root cwd is "".
    candidates.push(PathBuf::from("../../models/routelet"));
    candidates.push(PathBuf::from("models/routelet"));

    candidates
        .into_iter()
        .find(|p| p.join("embedder.onnx").exists())
}

/// OS keychain service the console stores the user's own provider keys
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
/// Mirrors aegis/src/providers/device_id.rs so the console and the agent
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

/// Base URL of the aegis proxy Worker. Override with AEGIS_PROXY_BASE (e.g.
/// http://localhost:8787) to point at a local `wrangler dev` while testing.
fn proxy_base() -> String {
    std::env::var("AEGIS_PROXY_BASE")
        .unwrap_or_else(|_| "https://aegis-proxy.danielbusnz.workers.dev".to_string())
}

/// Keychain accounts (under KEYRING_SERVICE) for the signed-in session. The
/// JWT is the credential; the email is cached only so the UI can show who is
/// signed in without decoding the token.
const SESSION_JWT_ACCOUNT: &str = "session_jwt";
const SESSION_EMAIL_ACCOUNT: &str = "session_email";

fn keychain_set(account: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| format!("save {account}: {e}"))
}

#[derive(serde::Serialize)]
struct Account {
    email: Option<String>,
    name: Option<String>,
}

#[derive(serde::Serialize)]
struct SessionStatus {
    signed_in: bool,
    email: Option<String>,
}

/// Sign in with GitHub. Opens the system browser at the proxy's OAuth start
/// endpoint, then polls the session endpoint until the proxy parks our JWT
/// (the proxy holds the OAuth client secret; this side only sees the final
/// session token). On success the token is stored in the OS keychain.
#[tauri::command]
async fn github_sign_in() -> Result<Account, String> {
    let state = uuid::Uuid::new_v4().to_string();
    let base = proxy_base();

    open::that(format!("{base}/auth/github/start?state={state}"))
        .map_err(|e| format!("couldn't open browser: {e}"))?;

    let client = reqwest::Client::new();
    let session_url = format!("{base}/auth/github/session?state={state}");

    // Poll for up to ~2 minutes while the user completes the browser flow.
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let resp = match client.get(&session_url).send().await {
            Ok(r) => r,
            Err(_) => continue, // transient network blip; keep polling
        };
        if resp.status().as_u16() == 404 {
            return Err("sign-in link expired, try again".to_string());
        }
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        if body.get("status").and_then(|v| v.as_str()) == Some("done") {
            let token = body.get("token").and_then(|v| v.as_str()).unwrap_or_default();
            if token.is_empty() {
                return Err("sign-in failed: empty token".to_string());
            }
            let email = body.get("email").and_then(|v| v.as_str()).map(str::to_string);
            let name = body.get("name").and_then(|v| v.as_str()).map(str::to_string);
            keychain_set(SESSION_JWT_ACCOUNT, token)?;
            if let Some(e) = &email {
                keychain_set(SESSION_EMAIL_ACCOUNT, e)?;
            }
            return Ok(Account { email, name });
        }
    }
    Err("sign-in timed out, try again".to_string())
}

/// Whether a session token is stored, plus the cached email for display.
#[tauri::command]
fn account_status() -> SessionStatus {
    SessionStatus {
        signed_in: keychain_get(SESSION_JWT_ACCOUNT).is_some(),
        email: keychain_get(SESSION_EMAIL_ACCOUNT),
    }
}

/// Forget the stored session (token + cached email).
#[tauri::command]
fn sign_out() -> Result<(), String> {
    for account in [SESSION_JWT_ACCOUNT, SESSION_EMAIL_ACCOUNT] {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, account) {
            let _ = entry.delete_credential();
        }
    }
    Ok(())
}

/// Per-integration status for the settings UI. Shells out to the aegis binary's
/// `integrations-status` subcommand (which runs the same health probes the
/// agent uses) and returns its JSON array of `{name, state, detail}`. The child
/// inherits this process's env, so e.g. the Gmail OAuth client id/secret are
/// visible to the probe.
#[tauri::command]
async fn integrations_status() -> Result<serde_json::Value, String> {
    let path = aegis_candidates()
        .into_iter()
        .find(|p| p.exists())
        .ok_or("aegis binary not found")?;

    let output = tokio::process::Command::new(path)
        .arg("integrations-status")
        .output()
        .await
        .map_err(|e| format!("failed to run aegis: {e}"))?;

    if !output.status.success() {
        return Err(format!("aegis integrations-status exited with {}", output.status));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| format!("bad status json: {e}"))
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
    // Dev escape hatch: AEGIS_SHOW_SIGNIN=1 forces the sign-in window so the
    // login flow can be exercised without going through (or resetting)
    // onboarding. Skips the onboarded-spawn shortcut below.
    let show_signin = std::env::var_os("AEGIS_SHOW_SIGNIN").is_some();

    // If already onboarded, spawn aegis directly and exit (no UI).
    if !show_signin && is_onboarded() {
        if let Err(e) = spawn_aegis() {
            eprintln!("[console] {e}");
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

    let builder = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            spawn_aegis,
            save_invite_code,
            mark_onboarded,
            verify_invite_code,
            save_api_keys,
            api_keys_status,
            verify_api_keys,
            github_sign_in,
            account_status,
            sign_out,
            integrations_status
        ])
        .setup(move |app| {
            // With the dev flag, surface the (normally hidden) settings window
            // and hide the onboarding window, so the console opens straight to login.
            if show_signin {
                if let Some(settings) = app.get_webview_window("settings") {
                    let _ = settings.show();
                    let _ = settings.set_focus();
                }
                if let Some(onboarding) = app.get_webview_window("onboarding") {
                    let _ = onboarding.hide();
                }
            }
            Ok(())
        });

    // macOS only: the permission plugin lets onboarding prompt for mic, screen
    // recording, and accessibility before the agent spawns. Compiled out
    // elsewhere, so the Linux/Windows builds are unchanged.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_macos_permissions::init());

    builder
        .run(tauri::generate_context!())
        .expect("error running console");
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
