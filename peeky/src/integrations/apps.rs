//! Generic app control via AppleScript (`osascript`). macOS only. Every macOS
//! app answers `activate` and `quit` regardless of whether it ships a real
//! scripting dictionary, so one module covers "open slack" and "quit zoom"
//! for anything installed. No install, no auth.

use super::applescript;

/// True on macOS, where `osascript` can address any installed application.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `app_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "app_open",
            "description": "Open (launch or bring to front) a macOS application by \
                name. Use for 'open slack', 'launch zoom', 'switch to chrome'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "app": {
                        "type": "string",
                        "description": "The application name as it appears in /Applications, \
                            e.g. 'Slack', 'Google Chrome'."
                    }
                },
                "required": ["app"]
            }
        }),
        serde_json::json!({
            "name": "app_quit",
            "description": "Quit a running macOS application by name. Use for \
                'quit zoom', 'close spotify'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "app": {
                        "type": "string",
                        "description": "The application name, e.g. 'Zoom', 'Spotify'."
                    }
                },
                "required": ["app"]
            }
        }),
        serde_json::json!({
            "name": "app_list_running",
            "description": "List the names of all running apps with a visible UI. \
                Use for 'what apps are open', 'what's running'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "app_open" => Some(match input["app"].as_str() {
            Some(app) => open(app),
            None => err_body("app_open missing 'app' field"),
        }),
        "app_quit" => Some(match input["app"].as_str() {
            Some(app) => quit(app),
            None => err_body("app_quit missing 'app' field"),
        }),
        "app_list_running" => Some(list_running()),
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

/// Fire-and-forget: launch the app if needed and bring it to the front. An
/// unknown name fails inside osascript ("Application can't be found"), which
/// flows back to Claude as the error body. Returns `{}` on success.
fn open(app: &str) -> String {
    let script = format!(
        "tell application \"{}\" to activate",
        applescript::escape(app)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("app_open failed: {e}")),
    }
}

/// Fire-and-forget: ask the app to quit (normal quit, the app may prompt to
/// save). Returns `{}` on success.
fn quit(app: &str) -> String {
    let script = format!("tell application \"{}\" to quit", applescript::escape(app));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("app_quit failed: {e}")),
    }
}

/// Data-returning: visible (non background-only) processes as
/// `{"apps":["..."]}`. One name per line from the script, split apart here.
fn list_running() -> String {
    let script = r#"set out to ""
tell application "System Events"
    repeat with p in (every process whose background only is false)
        set out to out & (name of p) & linefeed
    end repeat
end tell
return out"#;

    match applescript::run(script) {
        Ok(raw) => parse_app_lines(&raw),
        Err(e) => err_body(&format!("app_list_running failed: {e}")),
    }
}

/// Parse one-name-per-line output into `{"apps":[...]}`. Split out from
/// `list_running` so it can be unit-tested without running osascript.
fn parse_app_lines(raw: &str) -> String {
    let apps: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    serde_json::json!({ "apps": apps }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS and a GUI session, so they are
    // verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_three_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["app_open", "app_quit", "app_list_running"]);
    }

    #[test]
    fn dispatch_missing_app_returns_error_not_panic() {
        for tool in ["app_open", "app_quit"] {
            let out = dispatch(tool, &serde_json::json!({})).expect("dispatch owns the tool");
            assert!(out.contains("error"), "expected error body, got {out}");
            assert!(out.contains("missing"));
        }
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_an_app_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_app_lines_builds_one_entry_per_line() {
        let parsed: serde_json::Value =
            serde_json::from_str(&parse_app_lines("Safari\nMail\n")).unwrap();
        let apps = parsed["apps"].as_array().unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0], "Safari");
        assert_eq!(apps[1], "Mail");
    }

    #[test]
    fn parse_app_lines_handles_empty() {
        let parsed: serde_json::Value = serde_json::from_str(&parse_app_lines("")).unwrap();
        assert_eq!(parsed["apps"].as_array().unwrap().len(), 0);
    }
}
