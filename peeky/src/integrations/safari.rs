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

/// Fire-and-forget: open the URL in Safari. Returns `{}` on success.
fn open_url(url: &str) -> String {
    let script = format!(
        "tell application \"Safari\" to open location \"{}\"",
        applescript::escape(url)
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

    match applescript::run(script) {
        Ok(raw) => parse_tab_lines(&raw),
        Err(e) => err_body(&format!("safari list_tabs failed: {e}")),
    }
}

/// Parse the `url<tab>title` lines `list_tabs` produces into
/// `{"tabs":[{"url","title"}]}`. Split out from `list_tabs` so it can be
/// unit-tested without running osascript.
fn parse_tab_lines(raw: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions (open_url, current_tab, close_tab, the run
    // half of list_tabs) need macOS and a running Safari, so they are verified by
    // hand, not here. These cover the pure logic that runs on any platform.

    #[test]
    fn tools_exposes_the_four_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "safari_open_url",
                "safari_current_tab",
                "safari_list_tabs",
                "safari_close_tab"
            ]
        );
    }

    #[test]
    fn open_url_requires_a_url_argument() {
        let open = tools()
            .into_iter()
            .find(|t| t["name"] == "safari_open_url")
            .expect("safari_open_url tool present");
        assert_eq!(open["input_schema"]["required"][0], "url");
    }

    #[test]
    fn dispatch_missing_url_returns_error_not_panic() {
        let out = dispatch("safari_open_url", &serde_json::json!({}))
            .expect("dispatch owns safari_open_url");
        assert!(out.contains("error"), "expected an error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_safari_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_tab_lines_builds_one_entry_per_line() {
        let raw = "https://a.com\tSite A\nhttps://b.com\tSite B\n";
        let parsed: serde_json::Value = serde_json::from_str(&parse_tab_lines(raw)).unwrap();
        let tabs = parsed["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0]["url"], "https://a.com");
        assert_eq!(tabs[0]["title"], "Site A");
        assert_eq!(tabs[1]["title"], "Site B");
    }

    #[test]
    fn parse_tab_lines_handles_empty_and_titleless() {
        let empty: serde_json::Value = serde_json::from_str(&parse_tab_lines("")).unwrap();
        assert_eq!(empty["tabs"].as_array().unwrap().len(), 0);

        // A line with a URL but no tab/title still parses, title is empty.
        let no_title: serde_json::Value =
            serde_json::from_str(&parse_tab_lines("https://a.com\n")).unwrap();
        assert_eq!(no_title["tabs"][0]["url"], "https://a.com");
        assert_eq!(no_title["tabs"][0]["title"], "");
    }
}
