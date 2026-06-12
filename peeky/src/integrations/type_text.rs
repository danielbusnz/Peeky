//! Keyboard text injection as an integration tool. Unlike the AppleScript
//! integrations this is not app-scoped: it types into whatever field has
//! focus, through the same serialized input executor the find_action path
//! uses, so it lands after any pending click or app_open that came before it.

/// True where the input backend can synthesize keystrokes: ydotool on Linux,
/// CoreGraphics on macOS. The Windows backend is still a stub.
pub fn is_available() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// JSON tool schema Claude sees.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "type_text",
        "description": "Type text into the currently focused field, as if typed \
            on the keyboard. Use for 'type X', 'write X in this box', 'enter my \
            name in the field'. To type into a specific app, call app_open for \
            it first. End the text with a newline to press Enter after typing \
            (submit a search, send a message).",
        "input_schema": {
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to type. A trailing \\n presses Enter after."
                }
            },
            "required": ["text"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "type_text" => Some(match input["text"].as_str() {
            Some(text) => {
                crate::actions::type_text(text);
                "{}".to_string()
            }
            None => err_body("type_text missing 'text' field"),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Actually injecting keystrokes needs a display server, so the executor
    // call is verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["type_text"]);
    }

    #[test]
    fn dispatch_missing_text_returns_error_not_panic() {
        let out = dispatch("type_text", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_typing_tool", &serde_json::json!({})).is_none());
    }
}
