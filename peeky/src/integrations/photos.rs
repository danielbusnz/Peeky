//! Photos integration via AppleScript (`osascript`). macOS only. Shows an
//! album in the Photos app. No install, no auth.
//!
//! One tool on purpose: `spotlight` (show in app) is the verified, reliable
//! corner of the Photos dictionary. Slideshow and import can join later once
//! hand-tested on the Mac.

use super::applescript;

/// True on macOS, where `osascript` can drive Photos (ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `photos_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "photos_show_album",
        "description": "Open the Photos app and show an album by name. Use for \
            'show me my vacation photos', 'open the dogs album'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "album": {
                    "type": "string",
                    "description": "The album name, e.g. 'Vacation 2025'."
                }
            },
            "required": ["album"]
        }
    })]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "photos_show_album" => Some(match input["album"].as_str() {
            Some(album) => show_album(album),
            None => err_body("photos_show_album missing 'album' field"),
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

/// Fire-and-forget: bring Photos forward and spotlight (display) the album.
/// An unknown album fails inside osascript, which flows back to Claude.
/// Returns `{}` on success.
fn show_album(album: &str) -> String {
    let script = format!(
        "tell application \"Photos\"\n\
         activate\n\
         spotlight album \"{}\"\n\
         end tell",
        applescript::escape(album)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("photos_show_album failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed show needs macOS with a Photos library, so it is
    // verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_name() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["photos_show_album"]);
    }

    #[test]
    fn dispatch_missing_album_returns_error_not_panic() {
        let out =
            dispatch("photos_show_album", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_photos_tool", &serde_json::json!({})).is_none());
    }
}
