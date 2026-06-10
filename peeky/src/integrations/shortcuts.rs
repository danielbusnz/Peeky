//! Shortcuts integration via AppleScript (`osascript`). macOS only. Runs and
//! lists the user's Shortcuts. No install, no auth.
//!
//! Scripts target `Shortcuts Events`, the faceless helper that runs shortcuts
//! in the background; telling `Shortcuts` itself would open the app window.
//! This is the force multiplier integration: any shortcut the user has built,
//! including ones touching apps with no scripting dictionary (HomeKit,
//! Settings toggles), becomes a voice command.

use super::applescript;

/// True on macOS, where `osascript` can drive Shortcuts Events (Shortcuts
/// ships with macOS 12+, older versions fail at dispatch with a clear error).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `shortcuts_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "shortcuts_run",
            "description": "Run one of the user's Shortcuts by exact name, in the \
                background. Use when the user names a shortcut, e.g. 'run my \
                morning routine shortcut'. Use shortcuts_list first if unsure of \
                the exact name.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The exact shortcut name, e.g. 'Morning Routine'."
                    },
                    "input": {
                        "type": "string",
                        "description": "Optional text input passed to the shortcut."
                    }
                },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "shortcuts_list",
            "description": "List the names of all the user's Shortcuts. Use for \
                'what shortcuts do I have', or to find the exact name before \
                shortcuts_run.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "shortcuts_run" => Some(match input["name"].as_str() {
            Some(shortcut) => run(shortcut, input["input"].as_str()),
            None => err_body("shortcuts_run missing 'name' field"),
        }),
        "shortcuts_list" => Some(list()),
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

/// Run the named shortcut, optionally with text input. A shortcut's output
/// (if any) comes back as osascript stdout, which we surface as `{"result"}`
/// so Claude can speak it. Returns `{}` when the shortcut produced no output.
fn run(name: &str, input: Option<&str>) -> String {
    let script = match input {
        Some(text) => format!(
            "tell application \"Shortcuts Events\" to run shortcut named \"{}\" with input \"{}\"",
            applescript::escape(name),
            applescript::escape(text)
        ),
        None => format!(
            "tell application \"Shortcuts Events\" to run shortcut named \"{}\"",
            applescript::escape(name)
        ),
    };
    match applescript::run(&script) {
        Ok(out) if out.is_empty() => "{}".to_string(),
        Ok(out) => serde_json::json!({ "result": out }).to_string(),
        Err(e) => err_body(&format!("shortcuts_run failed: {e}")),
    }
}

/// Data-returning: every shortcut name as `{"shortcuts":["..."]}`. One name
/// per line from the script, split apart here.
fn list() -> String {
    let script = r#"set out to ""
tell application "Shortcuts Events"
    repeat with s in shortcuts
        set out to out & (name of s) & linefeed
    end repeat
end tell
return out"#;

    match applescript::run(script) {
        Ok(raw) => parse_shortcut_lines(&raw),
        Err(e) => err_body(&format!("shortcuts_list failed: {e}")),
    }
}

/// Parse one-name-per-line output into `{"shortcuts":[...]}`. Split out from
/// `list` so it can be unit-tested without running osascript.
fn parse_shortcut_lines(raw: &str) -> String {
    let shortcuts: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    serde_json::json!({ "shortcuts": shortcuts }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS with Shortcuts, so they are
    // verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_two_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["shortcuts_run", "shortcuts_list"]);
    }

    #[test]
    fn dispatch_missing_name_returns_error_not_panic() {
        let out =
            dispatch("shortcuts_run", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_shortcuts_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_shortcut_lines_builds_one_entry_per_line() {
        let parsed: serde_json::Value =
            serde_json::from_str(&parse_shortcut_lines("Morning Routine\nLog Water\n")).unwrap();
        let shortcuts = parsed["shortcuts"].as_array().unwrap();
        assert_eq!(shortcuts.len(), 2);
        assert_eq!(shortcuts[0], "Morning Routine");
    }

    #[test]
    fn parse_shortcut_lines_handles_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&parse_shortcut_lines("")).unwrap();
        assert_eq!(parsed["shortcuts"].as_array().unwrap().len(), 0);
    }
}
