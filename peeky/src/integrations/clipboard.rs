//! Clipboard integration via AppleScript (`osascript`). macOS only. Reads and
//! writes the system clipboard through Standard Additions' `the clipboard`.
//! No install, no auth.
//!
//! Reads coerce with `as text`, so a non-text clipboard (an image, a file)
//! fails inside osascript and the error flows back to Claude instead of
//! returning binary garbage.

use super::applescript;

/// True on macOS, where `osascript` can reach the clipboard.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `clipboard_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "clipboard_read",
            "description": "Read the current text on the clipboard. Use for \
                'what's on my clipboard', 'read me what I copied'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "clipboard_write",
            "description": "Put text on the clipboard, replacing what is there. \
                Use for 'copy that to my clipboard', 'put X on the clipboard'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The text to place on the clipboard." }
                },
                "required": ["text"]
            }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "clipboard_read" => Some(read()),
        "clipboard_write" => Some(match input["text"].as_str() {
            Some(text) => write(text),
            None => err_body("clipboard_write missing 'text' field"),
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

/// Data-returning: the clipboard text as `{"text"}`.
fn read() -> String {
    match applescript::run("the clipboard as text") {
        Ok(text) => serde_json::json!({ "text": text }).to_string(),
        Err(e) => err_body(&format!("clipboard_read failed: {e}")),
    }
}

/// Fire-and-forget: replace the clipboard contents. Returns `{}` on success.
fn write(text: &str) -> String {
    let script = format!("set the clipboard to \"{}\"", applescript::escape(text));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("clipboard_write failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS, so they are verified by hand.
    // These cover the pure logic.

    #[test]
    fn tools_exposes_the_two_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["clipboard_read", "clipboard_write"]);
    }

    #[test]
    fn dispatch_missing_text_returns_error_not_panic() {
        let out =
            dispatch("clipboard_write", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_clipboard_tool", &serde_json::json!({})).is_none());
    }
}
