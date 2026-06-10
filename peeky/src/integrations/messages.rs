//! Messages integration via AppleScript (`osascript`). macOS only. Sends an
//! iMessage to a phone number or email handle. No install, no auth.
//!
//! Sending is the reliable half of the Messages dictionary, so that is all
//! this exposes; reacting to incoming messages is notoriously flaky and is
//! deliberately absent. The `service`/`buddy` terms are the long-standing
//! compatibility names that still resolve on current macOS.
//!
//! The recipient must be a raw handle (phone number or email). For "text mom",
//! Claude resolves the name with `contacts_lookup` first, then calls this.

use super::applescript;

/// True on macOS, where `osascript` can drive Messages (ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `messages_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "messages_send",
        "description": "Send an iMessage. Use for 'text mom I'm on my way', \
            'message dan the address'. The recipient must be a phone number or \
            email handle; resolve a contact name with contacts_lookup first. \
            This sends immediately.",
        "input_schema": {
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Phone number (e.g. +16175551234) or iMessage email handle."
                },
                "text": {
                    "type": "string",
                    "description": "The message body to send."
                }
            },
            "required": ["recipient", "text"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "messages_send" => {
            let recipient = match input["recipient"].as_str() {
                Some(r) => r,
                None => return Some(err_body("messages_send missing 'recipient' field")),
            };
            let text = match input["text"].as_str() {
                Some(t) => t,
                None => return Some(err_body("messages_send missing 'text' field")),
            };
            Some(send(recipient, text))
        }
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

/// Fire-and-forget: send `text` to `recipient` over the iMessage service.
/// Returns `{}` on success.
fn send(recipient: &str, text: &str) -> String {
    let script = format!(
        "tell application \"Messages\"\n\
         set targetService to 1st service whose service type = iMessage\n\
         set targetBuddy to buddy \"{}\" of targetService\n\
         send \"{}\" to targetBuddy\n\
         end tell",
        applescript::escape(recipient),
        applescript::escape(text)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("messages_send failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed send needs macOS with a signed-in iMessage account,
    // so it is verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["messages_send"]);
    }

    #[test]
    fn dispatch_missing_recipient_returns_error_not_panic() {
        let out = dispatch("messages_send", &serde_json::json!({ "text": "hi" }))
            .expect("dispatch owns messages_send");
        assert!(out.contains("missing 'recipient'"));
    }

    #[test]
    fn dispatch_missing_text_returns_error_not_panic() {
        let out = dispatch(
            "messages_send",
            &serde_json::json!({ "recipient": "+1555" }),
        )
        .expect("dispatch owns messages_send");
        assert!(out.contains("missing 'text'"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_messages_tool", &serde_json::json!({})).is_none());
    }
}
