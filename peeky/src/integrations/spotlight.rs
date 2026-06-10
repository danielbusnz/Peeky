//! Spotlight integration via the official `mdfind` CLI. macOS only. Finds
//! files by name anywhere Spotlight indexes. No install, no auth.
//!
//! The natural pair for `finder_reveal`: "find that tax pdf" resolves the path
//! here, then reveal shows it. Results are capped so a broad query ("photo")
//! cannot flood the tool result Claude has to read.

use std::process::Command;

/// Cap on returned paths. Spotlight can match thousands; Claude needs a
/// handful to pick from or to ask the user to narrow down.
const MAX_RESULTS: usize = 10;

/// True on macOS, where `mdfind` ships with the system.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `spotlight_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "spotlight_search",
        "description": "Search for files by name with Spotlight. Use for 'find \
            that tax pdf', 'where is my resume'. Returns up to 10 matching \
            paths; follow up with finder_reveal or finder_open on the right one.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Part of the file name, e.g. 'resume' or 'tax 2025'."
                }
            },
            "required": ["name"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "spotlight_search" => Some(match input["name"].as_str() {
            Some(query) => search(query),
            None => err_body("spotlight_search missing 'name' field"),
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

/// Data-returning: matching paths as `{"paths":[...],"truncated":bool}`.
/// `mdfind -name` takes the query as a plain argument, so no quoting or
/// escaping games: the model string cannot break out into shell.
fn search(query: &str) -> String {
    match Command::new("mdfind").args(["-name", query]).output() {
        Ok(out) if out.status.success() => parse_paths(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => err_body(&format!(
            "spotlight_search mdfind failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => err_body(&format!("spotlight_search spawn failed: {e}")),
    }
}

/// Parse mdfind's one-path-per-line output into `{"paths":[...],"truncated"}`,
/// capped at `MAX_RESULTS`. Split out from `search` so it can be unit-tested
/// without running mdfind.
fn parse_paths(raw: &str) -> String {
    let all: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    let truncated = all.len() > MAX_RESULTS;
    let paths = &all[..all.len().min(MAX_RESULTS)];
    serde_json::json!({ "paths": paths, "truncated": truncated }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The mdfind-backed search needs macOS, so it is verified by hand. These
    // cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["spotlight_search"]);
    }

    #[test]
    fn dispatch_missing_name_returns_error_not_panic() {
        let out =
            dispatch("spotlight_search", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_spotlight_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_paths_caps_at_max_and_flags_truncation() {
        let raw = (0..15)
            .map(|i| format!("/Users/d/file{i}.pdf"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: serde_json::Value = serde_json::from_str(&parse_paths(&raw)).unwrap();
        assert_eq!(parsed["paths"].as_array().unwrap().len(), MAX_RESULTS);
        assert_eq!(parsed["truncated"], true);
    }

    #[test]
    fn parse_paths_handles_few_and_none() {
        let parsed: serde_json::Value =
            serde_json::from_str(&parse_paths("/a/b.txt\n/c/d.txt\n")).unwrap();
        assert_eq!(parsed["paths"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["truncated"], false);

        let empty: serde_json::Value = serde_json::from_str(&parse_paths("")).unwrap();
        assert_eq!(empty["paths"].as_array().unwrap().len(), 0);
    }
}
