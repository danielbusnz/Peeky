//! Safari integration via AppleScript (`osascript`). macOS only: Safari exposes
//! no CLI, so there is a single backend. It controls the running Safari app,
//! opening URLs and reading tabs, with no install and no auth.

use super::applescript;

/// True on macOS, where `osascript` can drive Safari. There is no CLI backend,
/// so Safari is AppleScript-only. Re-checked every agent-loop iteration.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `safari_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "safari_open_url",
            "description": "Open a URL in Safari (new tab). Use when the user wants to \
                visit or open a website, e.g. 'open github', 'go to nytimes.com'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The full URL including scheme, e.g. https://github.com"
                    }
                },
                "required": ["url"]
            }
        }),
        serde_json::json!({
            "name": "safari_current_tab",
            "description": "Get the URL and title of the active Safari tab. Use for \
                'what page am I on', 'what is this tab'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "safari_list_tabs",
            "description": "List the title and URL of every open tab in the front Safari \
                window. Use for 'what tabs do I have open'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "safari_close_tab",
            "description": "Close the active Safari tab.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "safari_open_url" => Some(match input["url"].as_str() {
            Some(url) => open_url(url),
            None => err_body("safari_open_url missing 'url' field"),
        }),
        "safari_current_tab" => Some(current_tab()),
        "safari_list_tabs" => Some(list_tabs()),
        "safari_close_tab" => Some(close_tab()),
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

/// Escape a value before it goes inside an AppleScript double-quoted string.
/// The `url` is model-provided, so a stray quote or backslash would otherwise
/// break the script (or inject into it).
fn escaped(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Fire-and-forget: open the URL in Safari. Returns `{}` on success.
fn open_url(url: &str) -> String {
    let script = format!(
        "tell application \"Safari\" to open location \"{}\"",
        escaped(url)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("safari open_url failed: {e}")),
    }
}

/// Data-returning: the URL and title of the active tab, as `{"url","title"}`.
/// Two shell-outs (one per property) keeps the AppleScript trivial and avoids
/// parsing a delimiter out of one combined string.
fn current_tab() -> String {
    let url =
        match applescript::run("tell application \"Safari\" to URL of current tab of front window")
        {
            Ok(u) => u,
            Err(e) => return err_body(&format!("safari current_tab url failed: {e}")),
        };
    let title = match applescript::run(
        "tell application \"Safari\" to name of current tab of front window",
    ) {
        Ok(t) => t,
        Err(e) => return err_body(&format!("safari current_tab title failed: {e}")),
    };
    serde_json::json!({ "url": url, "title": title }).to_string()
}

/// Data-returning: every tab in the front window as `{"tabs":[{"url","title"}]}`.
/// The script emits one `url<tab>title` line per tab using AppleScript's `tab`
/// and `linefeed` constants, which we split back apart here.
fn list_tabs() -> String {
    let script = r#"tell application "Safari"
    set out to ""
    repeat with t in tabs of front window
        set out to out & (URL of t) & tab & (name of t) & linefeed
    end repeat
    return out
end tell"#;

    let raw = match applescript::run(script) {
        Ok(r) => r,
        Err(e) => return err_body(&format!("safari list_tabs failed: {e}")),
    };

    let tabs: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(2, '\t');
            let url = parts.next().unwrap_or("");
            let title = parts.next().unwrap_or("");
            serde_json::json!({ "url": url, "title": title })
        })
        .collect();

    serde_json::json!({ "tabs": tabs }).to_string()
}

/// Fire-and-forget: close the active tab. Returns `{}` on success.
fn close_tab() -> String {
    match applescript::run("tell application \"Safari\" to close current tab of front window") {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("safari close_tab failed: {e}")),
    }
}
