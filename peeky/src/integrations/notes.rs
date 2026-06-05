//! Notes integration via AppleScript (`osascript`). macOS only. Creates a note
//! in the default account/folder. Notes derives the title from the first line of
//! the body. No install, no auth.

use super::applescript;

/// True on macOS, where `osascript` can drive Notes. No app install needed
/// (Notes ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `notes_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "notes_create",
        "description": "Create a note in the Notes app. Use for 'make a note', \
            'note that X', 'write down Y'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The note content. The first line becomes the title."
                }
            },
            "required": ["text"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "notes_create" => Some(match input["text"].as_str() {
            Some(text) => create(text),
            None => err_body("notes_create missing 'text' field"),
        }),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so failures reach Claude as tool_result
/// content, matching the shape the other integrations use.
fn err_body(msg: &str) -> String {
    format!(
        r#"{{"error":{}}}"#,
        serde_json::Value::String(msg.to_string())
    )
}

/// Create a note whose body is `text`. The doubled braces are format!'s escape
/// for the literal `{ }` of AppleScript's property record.
fn create(text: &str) -> String {
    let script = format!(
        "tell application \"Notes\" to make new note with properties {{body:\"{}\"}}",
        applescript::escape(text)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("notes_create failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_without_text_is_error_not_panic() {
        let out = dispatch("notes_create", &serde_json::json!({})).unwrap();
        assert!(out.contains("error"), "expected error body, got {out}");
    }

    #[test]
    fn dispatch_unknown_returns_none() {
        assert!(dispatch("not_a_notes_tool", &serde_json::json!({})).is_none());
    }
}
