//! Mail integration via AppleScript (`osascript`). macOS only. Sends an email
//! and reads the unread count. The local complement to the Gmail integration:
//! no OAuth, works with whatever accounts Mail is signed into.
//!
//! `mail_send` builds the message invisible (`visible:false`) and sends in one
//! script, so no compose window flashes on screen.

use super::applescript;

/// True on macOS, where `osascript` can drive Mail (ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `mail_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "mail_send",
            "description": "Send an email from Apple Mail. Use for 'email dan the \
                notes'. The recipient must be an email address; resolve a contact \
                name with contacts_lookup first. This sends immediately.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Recipient email address."
                    },
                    "subject": {
                        "type": "string",
                        "description": "The subject line."
                    },
                    "body": {
                        "type": "string",
                        "description": "The plain-text message body."
                    }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        serde_json::json!({
            "name": "mail_unread_count",
            "description": "Get the number of unread emails in the Apple Mail \
                inbox. Use for 'how many unread emails do I have'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "mail_send" => {
            let to = match input["to"].as_str() {
                Some(t) => t,
                None => return Some(err_body("mail_send missing 'to' field")),
            };
            let subject = match input["subject"].as_str() {
                Some(s) => s,
                None => return Some(err_body("mail_send missing 'subject' field")),
            };
            let body = match input["body"].as_str() {
                Some(b) => b,
                None => return Some(err_body("mail_send missing 'body' field")),
            };
            Some(send(to, subject, body))
        }
        "mail_unread_count" => Some(unread_count()),
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

/// Fire-and-forget: compose invisibly and send. The doubled braces are
/// format!'s escape for the literal `{ }` of AppleScript's property records.
/// Returns `{}` on success.
fn send(to: &str, subject: &str, body: &str) -> String {
    let script = format!(
        "tell application \"Mail\"\n\
         set newMessage to make new outgoing message with properties \
         {{subject:\"{}\", content:\"{}\", visible:false}}\n\
         tell newMessage to make new to recipient at end of to recipients \
         with properties {{address:\"{}\"}}\n\
         send newMessage\n\
         end tell",
        applescript::escape(subject),
        applescript::escape(body),
        applescript::escape(to)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("mail_send failed: {e}")),
    }
}

/// Data-returning: the inbox unread count as `{"unread":N}`. osascript hands
/// back the number as a decimal string, parsed here.
fn unread_count() -> String {
    match applescript::run("tell application \"Mail\" to get unread count of inbox") {
        Ok(raw) => parse_unread(&raw),
        Err(e) => err_body(&format!("mail_unread_count failed: {e}")),
    }
}

/// Parse osascript's decimal-string count into `{"unread":N}`. Split out from
/// `unread_count` so it can be unit-tested without running osascript.
fn parse_unread(raw: &str) -> String {
    match raw.trim().parse::<i64>() {
        Ok(n) => serde_json::json!({ "unread": n }).to_string(),
        Err(_) => err_body(&format!("mail_unread_count unparseable count: {raw}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS with Mail signed in, so they
    // are verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_two_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["mail_send", "mail_unread_count"]);
    }

    #[test]
    fn dispatch_missing_fields_return_error_not_panic() {
        let missing_to = dispatch(
            "mail_send",
            &serde_json::json!({ "subject": "s", "body": "b" }),
        )
        .expect("dispatch owns mail_send");
        assert!(missing_to.contains("missing 'to'"));

        let missing_subject = dispatch(
            "mail_send",
            &serde_json::json!({ "to": "a@b.c", "body": "b" }),
        )
        .expect("dispatch owns mail_send");
        assert!(missing_subject.contains("missing 'subject'"));

        let missing_body = dispatch(
            "mail_send",
            &serde_json::json!({ "to": "a@b.c", "subject": "s" }),
        )
        .expect("dispatch owns mail_send");
        assert!(missing_body.contains("missing 'body'"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_mail_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_unread_handles_count_and_garbage() {
        assert_eq!(parse_unread("12\n"), r#"{"unread":12}"#);
        assert_eq!(parse_unread("0"), r#"{"unread":0}"#);
        assert!(parse_unread("not a number").contains("error"));
    }
}
