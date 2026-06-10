//! Finder integration via AppleScript (`osascript`). macOS only. Opens,
//! reveals, and trashes files and folders by POSIX path. No install, no auth.
//!
//! Finder natively speaks colon-separated HFS paths, so every script coerces
//! the model-provided slash path with `POSIX file`. `delete` in Finder-speak
//! means move to Trash (reversible); there is deliberately no empty-trash tool.

use super::applescript;

/// True on macOS, where `osascript` can drive Finder (always present).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `finder_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "finder_open",
            "description": "Open a file or folder in Finder. A folder opens as a \
                Finder window, a file opens in its default app. Use for 'open my \
                downloads folder', 'open that pdf'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute POSIX path, e.g. /Users/me/Downloads. \
                            A leading ~ is expanded."
                    }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "finder_reveal",
            "description": "Reveal (select) a file or folder in a Finder window \
                without opening it. Use for 'show me that file in finder', \
                'where is this file'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute POSIX path. A leading ~ is expanded."
                    }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "finder_trash",
            "description": "Move a file or folder to the Trash (reversible, does not \
                empty the Trash). Use for 'delete that file', 'trash the old build'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute POSIX path. A leading ~ is expanded."
                    }
                },
                "required": ["path"]
            }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    let run_with_path = |tool: &str, action: fn(&str) -> String| -> String {
        match input["path"].as_str() {
            Some(path) => action(&expand_tilde(path)),
            None => err_body(&format!("{tool} missing 'path' field")),
        }
    };
    match name {
        "finder_open" => Some(run_with_path("finder_open", open)),
        "finder_reveal" => Some(run_with_path("finder_reveal", reveal)),
        "finder_trash" => Some(run_with_path("finder_trash", trash)),
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

/// Expand a leading `~` to `$HOME`. `POSIX file` does not expand tildes, and
/// the model often produces `~/Downloads` for "my downloads folder".
fn expand_tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) => expand_tilde_in(path, &home),
        Err(_) => path.to_string(),
    }
}

/// Pure half of `expand_tilde`, split out so it is testable without touching
/// the process environment.
fn expand_tilde_in(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        path.to_string()
    }
}

/// Fire-and-forget: open the file or folder, then bring Finder forward so a
/// folder window is actually visible. Returns `{}` on success.
fn open(path: &str) -> String {
    let script = format!(
        "tell application \"Finder\"\nopen (POSIX file \"{}\")\nactivate\nend tell",
        applescript::escape(path)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("finder_open failed: {e}")),
    }
}

/// Fire-and-forget: select the item in a Finder window. Returns `{}` on success.
fn reveal(path: &str) -> String {
    let script = format!(
        "tell application \"Finder\"\nreveal (POSIX file \"{}\")\nactivate\nend tell",
        applescript::escape(path)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("finder_reveal failed: {e}")),
    }
}

/// Fire-and-forget: move the item to the Trash. Finder's `delete` is the
/// reversible move-to-Trash, not a permanent removal. Returns `{}` on success.
fn trash(path: &str) -> String {
    let script = format!(
        "tell application \"Finder\" to delete (POSIX file \"{}\")",
        applescript::escape(path)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("finder_trash failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS and a GUI session, so they are
    // verified by hand. These cover the pure logic that runs on any platform.

    #[test]
    fn tools_exposes_the_three_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["finder_open", "finder_reveal", "finder_trash"]);
    }

    #[test]
    fn dispatch_missing_path_returns_error_not_panic() {
        for tool in ["finder_open", "finder_reveal", "finder_trash"] {
            let out = dispatch(tool, &serde_json::json!({})).expect("dispatch owns the tool");
            assert!(out.contains("error"), "expected error body, got {out}");
            assert!(out.contains("missing"));
        }
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_finder_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn expand_tilde_in_covers_bare_prefixed_and_absolute() {
        assert_eq!(expand_tilde_in("~", "/Users/d"), "/Users/d");
        assert_eq!(
            expand_tilde_in("~/Downloads", "/Users/d"),
            "/Users/d/Downloads"
        );
        assert_eq!(expand_tilde_in("/tmp/x", "/Users/d"), "/tmp/x");
        // A mid-path tilde is a literal file name, not an expansion target.
        assert_eq!(expand_tilde_in("/tmp/~x", "/Users/d"), "/tmp/~x");
    }
}
