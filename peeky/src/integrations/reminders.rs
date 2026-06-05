//! Reminders integration via AppleScript (`osascript`). macOS only. Creates a
//! reminder in the default list. No install, no auth.

use super::applescript;

/// True on macOS, where `osascript` can drive Reminders. No app install needed
/// (Reminders ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `reminders_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "reminders_add",
        "description": "Add a reminder to the default Reminders list. Use for \
            'remind me to X', 'add a reminder to Y'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The reminder text, e.g. 'buy milk'." }
            },
            "required": ["text"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "reminders_add" => Some(match input["text"].as_str() {
            Some(text) => add(text),
            None => err_body("reminders_add missing 'text' field"),
        }),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so failures reach Claude as tool_result
/// content, matching the shape the other integrations use.
fn err_body(msg: &str) -> String {
    format!(r#"{{"error":{}}}"#, serde_json::Value::String(msg.to_string()))
}

/// Create a reminder named `text` in the default list. The doubled braces are
/// format!'s escape for the literal `{ }` of AppleScript's property record.
fn add(text: &str) -> String {
    let script = format!(
        "tell application \"Reminders\" to make new reminder with properties {{name:\"{}\"}}",
        applescript::escape(text)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("reminders_add failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_without_text_is_error_not_panic() {
        let out = dispatch("reminders_add", &serde_json::json!({})).unwrap();
        assert!(out.contains("error"), "expected error body, got {out}");
    }

    #[test]
    fn dispatch_unknown_returns_none() {
        assert!(dispatch("not_a_reminders_tool", &serde_json::json!({})).is_none());
    }
}
