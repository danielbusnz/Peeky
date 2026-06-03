//! Low-level OS keychain access. All console secrets (the user's BYOK provider
//! keys and the signed-in session token) live under one service, keyed by an
//! account string. On Linux this is the Secret Service over DBus, which isn't
//! present on every setup; callers that must work without it should treat these
//! as best-effort and keep a file fallback.

/// Keychain service every console secret is stored under. Each secret is a
/// separate account within it.
const KEYRING_SERVICE: &str = "com.aegis.settings";

/// The stored value for `account`, or None if absent, blank, or unreadable.
pub(crate) fn keychain_get(account: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).ok()?;
    match entry.get_password() {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => None,
    }
}

/// Store `value` under `account`. Errors carry the account name for context.
pub(crate) fn keychain_set(account: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
    entry
        .set_password(value)
        .map_err(|e| format!("save {account}: {e}"))
}

/// Delete `account` if present. Best-effort: a missing entry or unavailable
/// keychain is a no-op.
pub(crate) fn keychain_delete(account: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, account) {
        let _ = entry.delete_credential();
    }
}
