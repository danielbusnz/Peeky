//! System audio control via AppleScript (`osascript`). macOS only. Volume and
//! mute are system settings, not an app, so there is no install, auth, or window
//! involved.
//!
//! Screen brightness is deliberately absent: macOS exposes no AppleScript for
//! it (it needs key-code simulation or a separate CLI), so it is not a quick add
//! and does not belong in this module.

use super::applescript;

/// True on macOS, where `osascript` can set the system volume. No app required.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `system_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "system_set_volume",
            "description": "Set the system output volume, 0 (silent) to 100 (max). \
                Use for 'turn it up', 'turn it down', 'set volume to 50'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "level": { "type": "integer", "description": "Volume from 0 to 100." }
                },
                "required": ["level"]
            }
        }),
        serde_json::json!({
            "name": "system_mute",
            "description": "Mute system audio output.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_unmute",
            "description": "Unmute system audio output.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "system_set_volume" => Some(match input["level"].as_i64() {
            Some(level) => set_volume(level),
            None => err_body("system_set_volume missing integer 'level' field"),
        }),
        "system_mute" => Some(set_muted(true)),
        "system_unmute" => Some(set_muted(false)),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so failures reach Claude as tool_result
/// content, matching the shape the other integrations use.
fn err_body(msg: &str) -> String {
    format!(r#"{{"error":{}}}"#, serde_json::Value::String(msg.to_string()))
}

/// Set output volume, clamping the model's level into AppleScript's 0..=100.
fn set_volume(level: i64) -> String {
    let script = format!("set volume output volume {}", level.clamp(0, 100));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system set_volume failed: {e}")),
    }
}

/// `set volume with output muted` / `without output muted`.
fn set_muted(muted: bool) -> String {
    let clause = if muted { "with" } else { "without" };
    let script = format!("set volume {clause} output muted");
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system set_muted failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_exposes_volume_and_mute() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["system_set_volume", "system_mute", "system_unmute"]);
    }

    #[test]
    fn set_volume_without_level_is_error_not_panic() {
        let out = dispatch("system_set_volume", &serde_json::json!({})).unwrap();
        assert!(out.contains("error"), "expected error body, got {out}");
    }

    #[test]
    fn dispatch_unknown_returns_none() {
        assert!(dispatch("not_a_system_tool", &serde_json::json!({})).is_none());
    }
}
