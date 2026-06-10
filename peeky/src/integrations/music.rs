//! Music (Apple Music) integration via AppleScript (`osascript`). macOS only.
//! Transport control, current-track info, and library track search. The Apple
//! Music counterpart to the Spotify integration; both can be live at once and
//! Claude picks by what the user names. No install, no auth.
//!
//! `music_play_track` searches the local library only: the Music dictionary
//! cannot search the streaming catalog (same limitation as Spotify's
//! AppleScript backend).

use super::applescript;

/// True on macOS, where `osascript` can drive Music (ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `music_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "music_play",
            "description": "Resume playback in Apple Music. Use for 'play' / \
                'resume' when the user means Apple Music.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "music_pause",
            "description": "Pause Apple Music playback.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "music_next",
            "description": "Skip to the next track in Apple Music.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "music_previous",
            "description": "Go back to the previous track in Apple Music.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "music_current_track",
            "description": "Get the name and artist of the track now playing in \
                Apple Music. Use for 'what song is this'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "music_play_track",
            "description": "Search the user's Apple Music library for a track by \
                name and play the first match. Library only, not the streaming \
                catalog. Use for 'play bohemian rhapsody on apple music'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Part of the track name, e.g. 'bohemian rhapsody'."
                    }
                },
                "required": ["query"]
            }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "music_play" => Some(transport("play", "music_play")),
        "music_pause" => Some(transport("pause", "music_pause")),
        "music_next" => Some(transport("next track", "music_next")),
        "music_previous" => Some(transport("previous track", "music_previous")),
        "music_current_track" => Some(current_track()),
        "music_play_track" => Some(match input["query"].as_str() {
            Some(query) => play_track(query),
            None => err_body("music_play_track missing 'query' field"),
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

/// Fire-and-forget: one of the bare transport verbs (`play`, `pause`,
/// `next track`, `previous track`). The verb is a compile-time constant from
/// `dispatch`, never model text, so it goes into the script unescaped.
fn transport(verb: &str, tool: &str) -> String {
    let script = format!("tell application \"Music\" to {verb}");
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("{tool} failed: {e}")),
    }
}

/// Data-returning: the playing track as `{"playing",("track","artist")}`.
/// The `exists current track` guard turns the stopped state into a clean
/// `{"playing":false}` instead of an osascript error.
fn current_track() -> String {
    let script = r#"tell application "Music"
    if exists current track then
        return (name of current track) & tab & (artist of current track)
    else
        return ""
    end if
end tell"#;

    match applescript::run(script) {
        Ok(raw) if raw.is_empty() => serde_json::json!({ "playing": false }).to_string(),
        Ok(raw) => parse_track_line(&raw),
        Err(e) => err_body(&format!("music_current_track failed: {e}")),
    }
}

/// Parse the `name<tab>artist` line `current_track` produces into
/// `{"playing":true,"track","artist"}`. Split out so it can be unit-tested
/// without running osascript.
fn parse_track_line(raw: &str) -> String {
    let mut parts = raw.splitn(2, '\t');
    let track = parts.next().unwrap_or("");
    let artist = parts.next().unwrap_or("");
    serde_json::json!({ "playing": true, "track": track, "artist": artist }).to_string()
}

/// Fire-and-forget: play the first library track whose name matches. A no-match
/// query fails inside osascript ("Can't get track ..."), which flows back to
/// Claude as the error body so it can tell the user the song isn't in the
/// library.
fn play_track(query: &str) -> String {
    let script = format!(
        "tell application \"Music\" to play (first track whose name contains \"{}\")",
        applescript::escape(query)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("music_play_track failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS with Music, so they are
    // verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_six_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "music_play",
                "music_pause",
                "music_next",
                "music_previous",
                "music_current_track",
                "music_play_track"
            ]
        );
    }

    #[test]
    fn dispatch_missing_query_returns_error_not_panic() {
        let out =
            dispatch("music_play_track", &serde_json::json!({})).expect("dispatch owns the tool");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_music_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_track_line_splits_name_and_artist() {
        let parsed: serde_json::Value =
            serde_json::from_str(&parse_track_line("Karma Police\tRadiohead")).unwrap();
        assert_eq!(parsed["playing"], true);
        assert_eq!(parsed["track"], "Karma Police");
        assert_eq!(parsed["artist"], "Radiohead");
    }

    #[test]
    fn parse_track_line_handles_missing_artist() {
        let parsed: serde_json::Value =
            serde_json::from_str(&parse_track_line("Untitled")).unwrap();
        assert_eq!(parsed["track"], "Untitled");
        assert_eq!(parsed["artist"], "");
    }
}
