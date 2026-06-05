//! Invite codes and onboarding state. Validates codes locally and against the
//! proxy, persists the chosen code where the agent reads it, and tracks the
//! one-time "onboarded" marker that gates the first-run UI.

use crate::proxy_contract;

/// Local format check. Mirrors the proxy's CODE_RE so we fail on obvious junk
/// before the proxy ever sees it. The proxy is the source of truth for expiry,
/// device limits, and unknown codes.
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
/// Mirrors peeky/src/providers/device_id.rs so the console and the agent agree
/// on the same id at `~/.config/peeky/device_id`.
fn device_id() -> Result<String, String> {
    let path = dirs::config_dir()
        .ok_or("no config dir on this platform")?
        .join("peeky")
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
/// invite field. Format-checks locally for instant feedback, then asks the proxy
/// whether the code is real, unexpired, and has a device slot. `Ok` means usable
/// (green); `Err(message)` carries a reason to show (red).
#[tauri::command]
pub async fn verify_invite_code(code: String) -> Result<(), String> {
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

/// Validate and persist an invite code to the same config dir peeky reads from
/// at startup (see peeky/src/providers/invite_code.rs). The empty string clears
/// the code.
#[tauri::command]
pub fn save_invite_code(code: String) -> Result<(), String> {
    let trimmed = code.trim();
    if !trimmed.is_empty() {
        validate_code(trimmed).map_err(str::to_string)?;
    }

    let dir = dirs::config_dir()
        .ok_or("no config dir on this platform")?
        .join("peeky");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create_dir_all: {e}"))?;
    let path = dir.join("invite_code");
    std::fs::write(&path, trimmed).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// Whether first-run onboarding has already completed (marker file present).
pub(crate) fn is_onboarded() -> bool {
    dirs::config_dir()
        .map(|d| d.join("peeky").join("onboarded").exists())
        .unwrap_or(false)
}

/// Write the onboarded marker so the next launch skips the first-run UI.
#[tauri::command]
pub fn mark_onboarded() -> Result<(), String> {
    let dir = dirs::config_dir().ok_or("no config dir")?.join("peeky");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    std::fs::write(dir.join("onboarded"), "").map_err(|e| format!("{e}"))?;
    Ok(())
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
