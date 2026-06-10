//! Contacts integration via AppleScript (`osascript`). macOS only. Looks up
//! phone numbers and email addresses by name. No install, no auth.
//!
//! Mostly an enabler: `messages_send` and `mail_send` take raw handles, so
//! "text mom" resolves through here first. AppleScript's `whose name contains`
//! comparison is case-insensitive by default, which fits voice transcripts.

use super::applescript;

/// True on macOS, where `osascript` can drive Contacts (ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `contacts_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "contacts_lookup",
        "description": "Look up a person in Contacts by (partial) name and get \
            their phone numbers and email addresses. Use before messages_send or \
            mail_send when the user names a person, e.g. 'text mom' or \
            'email dan'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Full or partial contact name, e.g. 'mom', 'Dan Brooks'."
                }
            },
            "required": ["name"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "contacts_lookup" => Some(match input["name"].as_str() {
            Some(query) => lookup(query),
            None => err_body("contacts_lookup missing 'name' field"),
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

/// Data-returning: matches as `{"contacts":[{"name","phones","emails"}]}`.
/// The script emits one `name<tab>phones<tab>emails` line per person, with
/// `; `-joined values inside the phone and email fields, split apart here.
fn lookup(query: &str) -> String {
    let script = format!(
        r#"set out to ""
tell application "Contacts"
    repeat with p in (people whose name contains "{}")
        set phoneList to ""
        repeat with ph in phones of p
            set phoneList to phoneList & (value of ph) & "; "
        end repeat
        set emailList to ""
        repeat with em in emails of p
            set emailList to emailList & (value of em) & "; "
        end repeat
        set out to out & (name of p) & tab & phoneList & tab & emailList & linefeed
    end repeat
end tell
return out"#,
        applescript::escape(query)
    );

    match applescript::run(&script) {
        Ok(raw) => parse_contact_lines(&raw),
        Err(e) => err_body(&format!("contacts_lookup failed: {e}")),
    }
}

/// Parse the `name<tab>phones<tab>emails` lines `lookup` produces into
/// `{"contacts":[{"name","phones","emails"}]}`. The `; ` value joins from the
/// script are trimmed of their trailing separator. Split out from `lookup` so
/// it can be unit-tested without running osascript.
fn parse_contact_lines(raw: &str) -> String {
    let contacts: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(3, '\t');
            let name = parts.next().unwrap_or("");
            let phones = split_joined(parts.next().unwrap_or(""));
            let emails = split_joined(parts.next().unwrap_or(""));
            serde_json::json!({ "name": name, "phones": phones, "emails": emails })
        })
        .collect();

    serde_json::json!({ "contacts": contacts }).to_string()
}

/// Split a `; `-joined field back into its values, dropping the empties the
/// trailing separator leaves behind.
fn split_joined(field: &str) -> Vec<&str> {
    field
        .split("; ")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed lookup needs macOS with a populated Contacts, so it
    // is verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["contacts_lookup"]);
    }

    #[test]
    fn dispatch_missing_name_returns_error_not_panic() {
        let out =
            dispatch("contacts_lookup", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_contacts_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_contact_lines_splits_fields_and_values() {
        let raw = "Mom\t+16175551234; +16175555678; \tmom@example.com; \n";
        let parsed: serde_json::Value = serde_json::from_str(&parse_contact_lines(raw)).unwrap();
        let contacts = parsed["contacts"].as_array().unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0]["name"], "Mom");
        assert_eq!(contacts[0]["phones"].as_array().unwrap().len(), 2);
        assert_eq!(contacts[0]["phones"][0], "+16175551234");
        assert_eq!(contacts[0]["emails"][0], "mom@example.com");
    }

    #[test]
    fn parse_contact_lines_handles_empty_and_no_handles() {
        let empty: serde_json::Value = serde_json::from_str(&parse_contact_lines("")).unwrap();
        assert_eq!(empty["contacts"].as_array().unwrap().len(), 0);

        // A contact with no phones or emails still parses with empty arrays.
        let bare: serde_json::Value =
            serde_json::from_str(&parse_contact_lines("Dan\t\t\n")).unwrap();
        assert_eq!(bare["contacts"][0]["name"], "Dan");
        assert_eq!(bare["contacts"][0]["phones"].as_array().unwrap().len(), 0);
        assert_eq!(bare["contacts"][0]["emails"].as_array().unwrap().len(), 0);
    }
}
