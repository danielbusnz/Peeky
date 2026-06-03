//! Integration health for the settings UI. Shells out to the aegis binary's
//! `integrations-status` subcommand (the same probes the agent runs) so the
//! console doesn't reimplement them.

use crate::agent::aegis_candidates;

/// Per-integration status as a JSON array of `{name, state, detail}`. The child
/// inherits this process's env, so e.g. the Gmail OAuth client id/secret are
/// visible to the probe.
#[tauri::command]
pub async fn integrations_status() -> Result<serde_json::Value, String> {
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
        return Err(format!(
            "aegis integrations-status exited with {}",
            output.status
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| format!("bad status json: {e}"))
}
