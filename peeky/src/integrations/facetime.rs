//! FaceTime integration via Apple's official `facetime:` URL schemes (Apple
//! URL Scheme Reference), opened with Standard Additions' `open location`.
//! macOS only. No install, no auth.
//!
//! Safety property worth keeping: the URL only readies the call. FaceTime
//! itself asks the user to confirm before dialing, so a misheard "call mom"
//! cannot place a call on its own.

use super::applescript;

/// True on macOS, where `osascript` can open `facetime:` URLs.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `facetime_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "facetime_call",
        "description": "Start a FaceTime call (the user confirms in FaceTime \
            before it dials). Use for 'facetime mom', 'call dan on facetime'. \
            The recipient must be a phone number or email handle; resolve a \
            contact name with contacts_lookup first.",
        "input_schema": {
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Phone number (e.g. +16175551234) or FaceTime email handle."
                },
                "audio_only": {
                    "type": "boolean",
                    "description": "true for a FaceTime Audio call. Defaults to false (video)."
                }
            },
            "required": ["recipient"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "facetime_call" => Some(match input["recipient"].as_str() {
            Some(recipient) => call(recipient, input["audio_only"].as_bool().unwrap_or(false)),
            None => err_body("facetime_call missing 'recipient' field"),
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

/// Fire-and-forget: open the `facetime:` / `facetime-audio:` URL, which brings
/// up FaceTime with the call ready to confirm. Returns `{}` on success.
fn call(recipient: &str, audio_only: bool) -> String {
    let scheme = if audio_only {
        "facetime-audio"
    } else {
        "facetime"
    };
    let url = format!("{scheme}://{}", clean_handle(recipient));
    let script = format!("open location \"{}\"", applescript::escape(&url));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("facetime_call failed: {e}")),
    }
}

/// Strip the whitespace a spoken or contact-card number carries ("+1 617 555
/// 1234"), which would break the URL. Email handles pass through unchanged.
fn clean_handle(recipient: &str) -> String {
    recipient.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed call needs macOS with FaceTime signed in, so it is
    // verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["facetime_call"]);
    }

    #[test]
    fn dispatch_missing_recipient_returns_error_not_panic() {
        let out =
            dispatch("facetime_call", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_facetime_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn clean_handle_strips_spaces_and_keeps_emails() {
        assert_eq!(clean_handle("+1 617 555 1234"), "+16175551234");
        assert_eq!(clean_handle("dan@example.com"), "dan@example.com");
        assert_eq!(clean_handle("+16175551234"), "+16175551234");
    }
}
