//! Keynote integration via AppleScript (`osascript`). macOS only, and only
//! when Keynote is installed (free on the App Store but not preinstalled).
//! Drives the frontmost presentation: start, next, previous, stop. "Next
//! slide" by voice while presenting is the whole point.
//!
//! Commands follow the official Keynote suite: `start` / `stop` take the
//! front document, `show next` / `show previous` are application-level.

use super::applescript;

/// True on macOS with Keynote installed. Re-checked every agent-loop
/// iteration, so installing Keynote mid-session works without restarting.
pub fn is_available() -> bool {
    cfg!(target_os = "macos") && std::path::Path::new("/Applications/Keynote.app").exists()
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `keynote_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "keynote_start",
            "description": "Start presenting the frontmost Keynote document. Use \
                for 'start the presentation', 'present my slides'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "keynote_next",
            "description": "Advance to the next slide or build in the running \
                Keynote presentation. Use for 'next slide'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "keynote_previous",
            "description": "Go back to the previous slide in the running Keynote \
                presentation. Use for 'previous slide', 'go back a slide'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "keynote_stop",
            "description": "Stop the running Keynote presentation. Use for 'stop \
                presenting', 'end the slideshow'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, _input: &serde_json::Value) -> Option<String> {
    match name {
        "keynote_start" => Some(run_keynote(
            "activate\nstart front document",
            "keynote_start",
        )),
        "keynote_next" => Some(run_keynote("show next", "keynote_next")),
        "keynote_previous" => Some(run_keynote("show previous", "keynote_previous")),
        "keynote_stop" => Some(run_keynote("stop front document", "keynote_stop")),
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

/// Fire-and-forget: run `body` inside a Keynote tell block. The body is a
/// compile-time constant from `dispatch`, never model text, so it goes into
/// the script unescaped. No open document fails inside osascript, which flows
/// back to Claude. Returns `{}` on success.
fn run_keynote(body: &str, tool: &str) -> String {
    let script = format!("tell application \"Keynote\"\n{body}\nend tell");
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("{tool} failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed commands need macOS with Keynote and an open deck,
    // so they are verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_four_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "keynote_start",
                "keynote_next",
                "keynote_previous",
                "keynote_stop"
            ]
        );
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_keynote_tool", &serde_json::json!({})).is_none());
    }
}
