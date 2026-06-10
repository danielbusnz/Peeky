//! Maps integration via Apple's official map URL scheme (Apple URL Scheme
//! Reference: Map Links), opened with Standard Additions' `open location`.
//! macOS only. No install, no auth.
//!
//! `maps://` opens the Maps app directly with the documented `maps.apple.com`
//! query parameters: `daddr` for directions, `q` for a search.

use super::applescript;

/// True on macOS, where `osascript` can open `maps:` URLs.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `maps_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "maps_directions",
            "description": "Open Apple Maps with directions to a destination from \
                the current location. Use for 'directions to the airport', \
                'navigate to 123 main street'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "destination": {
                        "type": "string",
                        "description": "Address or place name, e.g. 'Boston Logan Airport'."
                    }
                },
                "required": ["destination"]
            }
        }),
        serde_json::json!({
            "name": "maps_search",
            "description": "Search Apple Maps for a place. Use for 'find coffee \
                near me', 'show thai restaurants on the map'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to search for, e.g. 'coffee'."
                    }
                },
                "required": ["query"]
            }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "maps_directions" => Some(match input["destination"].as_str() {
            Some(dest) => open_maps("daddr", dest, "maps_directions"),
            None => err_body("maps_directions missing 'destination' field"),
        }),
        "maps_search" => Some(match input["query"].as_str() {
            Some(query) => open_maps("q", query, "maps_search"),
            None => err_body("maps_search missing 'query' field"),
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

/// Fire-and-forget: open `maps://?<param>=<value>` with the value
/// percent-encoded. Returns `{}` on success.
fn open_maps(param: &str, value: &str, tool: &str) -> String {
    let url = format!("maps://?{param}={}", percent_encode(value));
    let script = format!("open location \"{}\"", applescript::escape(&url));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("{tool} failed: {e}")),
    }
}

/// Percent-encode everything but RFC 3986 unreserved characters, so a spoken
/// destination ("123 Main St, Boston") survives the trip through the URL.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed opens need macOS, so they are verified by hand.
    // These cover the pure logic.

    #[test]
    fn tools_exposes_the_two_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["maps_directions", "maps_search"]);
    }

    #[test]
    fn dispatch_missing_fields_return_error_not_panic() {
        for tool in ["maps_directions", "maps_search"] {
            let out = dispatch(tool, &serde_json::json!({})).expect("dispatch owns the tool");
            assert!(out.contains("error"), "expected error body, got {out}");
            assert!(out.contains("missing"));
        }
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_maps_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn percent_encode_covers_spaces_punctuation_and_unicode() {
        assert_eq!(percent_encode("plain"), "plain");
        assert_eq!(
            percent_encode("123 Main St, Boston"),
            "123%20Main%20St%2C%20Boston"
        );
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        // Multi-byte UTF-8 encodes per byte.
        assert_eq!(percent_encode("café"), "caf%C3%A9");
    }
}
