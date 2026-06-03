//! Signed-in session: GitHub OAuth sign-in (Worker-mediated), the session-token
//! store, and the status the settings UI reads. The token is the credential; it
//! lives in a 0600 file the agent reads (the source of truth) and, best-effort,
//! in the OS keychain to back the "signed in as X" display.

use crate::keychain::{keychain_delete, keychain_get, keychain_set};

/// Base URL of the aegis proxy Worker. Override with AEGIS_PROXY_BASE (e.g.
/// http://localhost:8787) to point at a local `wrangler dev` while testing.
fn proxy_base() -> String {
    std::env::var("AEGIS_PROXY_BASE")
        .unwrap_or_else(|_| "https://aegis-proxy.danielbusnz.workers.dev".to_string())
}

/// Keychain accounts for the signed-in session. The JWT is the credential; the
/// email is cached only so the UI can show who is signed in without decoding the
/// token.
const SESSION_JWT_ACCOUNT: &str = "session_jwt";
const SESSION_EMAIL_ACCOUNT: &str = "session_email";

/// The agent-readable session JWT file. This is the source of truth for "signed
/// in": the running agent reads it per request (see
/// aegis/src/providers/session_jwt.rs), and it works on Linux setups with no
/// Secret Service, unlike the keychain.
fn session_jwt_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("aegis").join("session_jwt"))
}

/// Write the session JWT to the 0600 file the agent reads, so signing in
/// upgrades the live session to the account tier without a restart.
fn write_session_jwt_file(token: &str) -> Result<(), String> {
    let path = session_jwt_path().ok_or("no config dir on this platform")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    std::fs::write(&path, token).map_err(|e| format!("write: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Remove the agent-readable session JWT file on sign-out. A missing file is
/// fine: the agent treats absent as "trial tier".
fn clear_session_jwt_file() {
    if let Some(path) = session_jwt_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[derive(serde::Serialize)]
pub struct Account {
    email: Option<String>,
    name: Option<String>,
}

#[derive(serde::Serialize)]
pub struct SessionStatus {
    signed_in: bool,
    email: Option<String>,
}

/// Sign in with GitHub. Opens the system browser at the proxy's OAuth start
/// endpoint, then polls the session endpoint until the proxy parks our JWT (the
/// proxy holds the OAuth client secret; this side only sees the final session
/// token). On success the token is written to the agent file (and the keychain,
/// best-effort).
#[tauri::command]
pub async fn github_sign_in() -> Result<Account, String> {
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
            // The file is what the running agent reads, so it's the required
            // write: a sign-in that doesn't land here didn't really happen.
            write_session_jwt_file(token)?;
            // Best-effort: the keychain only backs the console's signed-in
            // display, and isn't available on every Linux setup (no Secret
            // Service over DBus). Don't fail a working sign-in over it.
            let _ = keychain_set(SESSION_JWT_ACCOUNT, token);
            if let Some(e) = &email {
                let _ = keychain_set(SESSION_EMAIL_ACCOUNT, e);
            }
            return Ok(Account { email, name });
        }
    }
    Err("sign-in timed out, try again".to_string())
}

/// Whether a session token is stored, plus the cached email for display.
/// Signed-in tracks the agent-readable file (the source of truth, present even
/// where the keychain isn't); the email is a keychain-only nicety and may be
/// absent on setups without a Secret Service even when signed in.
#[tauri::command]
pub fn account_status() -> SessionStatus {
    let signed_in =
        session_jwt_path().is_some_and(|p| p.exists()) || keychain_get(SESSION_JWT_ACCOUNT).is_some();
    SessionStatus {
        signed_in,
        email: keychain_get(SESSION_EMAIL_ACCOUNT),
    }
}

/// Forget the stored session (token file + cached keychain entries).
#[tauri::command]
pub fn sign_out() -> Result<(), String> {
    keychain_delete(SESSION_JWT_ACCOUNT);
    keychain_delete(SESSION_EMAIL_ACCOUNT);
    clear_session_jwt_file();
    Ok(())
}
