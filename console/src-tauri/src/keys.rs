//! Bring-your-own-keys: the user's own provider API keys. Stored in the OS
//! keychain, injected into the peeky child via the env contract its providers
//! understand, and live-checked against each provider before they're trusted.

use std::process::Command;

use crate::keychain::{keychain_get, keychain_set};

/// (keychain account, peeky "go direct" flag, provider key env var) per
/// provider. The peeky runtime switches a provider to direct mode when its
/// DIRECT flag is present, reading the key from the matching var.
const BYOK_PROVIDERS: [(&str, &str, &str); 3] = [
    ("anthropic", "PEEKY_ANTHROPIC_DIRECT", "ANTHROPIC_API_KEY"),
    ("deepgram", "PEEKY_DEEPGRAM_DIRECT", "DEEPGRAM_API_KEY"),
    ("cartesia", "PEEKY_CARTESIA_DIRECT", "CARTESIA_API_KEY"),
];

/// Inject any stored BYOK keys into the peeky child using the env contract its
/// providers already understand. Providers with no stored key are left on the
/// proxy/trial path, so direct + trial can mix per provider.
pub(crate) fn apply_byok_env(cmd: &mut Command) {
    for (account, direct_flag, key_var) in BYOK_PROVIDERS {
        if let Some(key) = keychain_get(account) {
            cmd.env(direct_flag, "1").env(key_var, key);
        }
    }
}

/// Persist the user's own provider keys to the OS keychain. A blank value is
/// skipped, not cleared: the UI pre-fills a "saved" hint for providers that
/// already have a key, so submitting the form blank must keep them.
#[tauri::command]
pub fn save_api_keys(anthropic: String, deepgram: String, cartesia: String) -> Result<(), String> {
    let values = [anthropic, deepgram, cartesia];
    for ((account, _, _), value) in BYOK_PROVIDERS.iter().zip(values.iter()) {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        keychain_set(account, trimmed)?;
    }
    Ok(())
}

/// Which providers currently have a stored key. Booleans only, never the secret
/// values, so the UI can show "saved" state without exposing keys.
#[tauri::command]
pub fn api_keys_status() -> std::collections::HashMap<String, bool> {
    BYOK_PROVIDERS
        .iter()
        .map(|(account, _, _)| (account.to_string(), keychain_get(account).is_some()))
        .collect()
}

/// Live-validate the user's own provider keys by hitting each provider's auth
/// the same way peeky does in direct mode. Returns a per-provider map of whether
/// the key works. An empty key is reported `false`. Used by the onboarding gate
/// so a typo'd key can't slip through.
#[tauri::command]
pub async fn verify_api_keys(
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
