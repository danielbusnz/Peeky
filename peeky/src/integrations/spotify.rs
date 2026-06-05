//! Spotify integration via the `spotify_player` CLI.
//!
//! Setup the user does once:
//! ```text
//! cargo install spotify_player
//! spotify_player authenticate   # opens browser for OAuth
//! ```
//!
//! Requires Spotify Premium (Spotify's API forbids programmatic playback
//! on free tier). The desktop app or spotifyd must be running as the
//! active playback device.

use std::process::Command;

use crate::integrations::applescript;

/// True if either backend is usable: the AppleScript app path on macOS, or the
/// `spotify_player` CLI. Re-checked every agent-loop iteration so installing a
/// backend mid-session works without restarting peeky.
pub fn is_available() -> bool {
    applescript_backend() || cli_backend()
}

/// True on macOS, where `osascript` drives the Spotify desktop app with no
/// install. Other platforms fall back to the CLI backend.
fn applescript_backend() -> bool {
    cfg!(target_os = "macos")
}

/// True if the `spotify_player` CLI is on PATH. This is the search-capable
/// backend; AppleScript cannot search the catalog.
fn cli_backend() -> bool {
    Command::new("which")
        .arg("spotify_player")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// JSON tool schemas Claude sees. Each `name` must be globally unique
/// across all integrations, so prefix with `spotify_`.
pub fn tools() -> Vec<serde_json::Value> {
    let mut tools = vec![
        serde_json::json!({
            "name": "spotify_pause",
            "description": "Pause Spotify playback.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "spotify_resume",
            "description": "Resume Spotify playback after a pause.",
                "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "spotify_next",
            "description": "Skip to the next track on Spotify.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "spotify_previous",
            "description": "Go to the previous track on Spotify.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ];

    // Catalog search needs the CLI backend. AppleScript can only control the
    // running app, not search it, so advertise spotify_play only where it works.
    if cli_backend() {
        tools.push(serde_json::json!({
            "name": "spotify_play",
            "description": "Search Spotify and play the top result. Use this for ANY \
                'play X on Spotify' / 'play song X' intent when the user has Spotify \
                installed. Dramatically faster than visually clicking through the \
                Spotify UI. The query can be a song name, artist, album, or combination \
                (e.g. 'sicko mode travis scott'). Requires Spotify Premium.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query: song name, artist, album, or combination."
                    }
                },
                "required": ["query"]
            }
        }));
    }

    tools
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "spotify_play" => Some(match input["query"].as_str() {
            Some(q) => play(q),
            None => err_body("spotify_play missing 'query' field"),
        }),
        "spotify_pause" => Some(control("pause")),
        // spotify_player calls this "play", not "resume".
        "spotify_resume" => Some(control("play")),
        "spotify_next" => Some(control("next")),
        "spotify_previous" => Some(control("previous")),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so spotify failures reach Claude as
/// tool_result content. Matches github.rs's convention so the agent loop
/// sees a consistent error shape across integrations.
fn err_body(msg: &str) -> String {
    format!(
        r#"{{"error":{}}}"#,
        serde_json::Value::String(msg.to_string())
    )
}

/// Search for `query`, take the first track ID from the result, and start
/// playback. Two shell-outs: spotify_player's CLI doesn't (as of this
/// writing) expose a single "search and play top result" command. If the
/// search output format isn't what `extract_first_track_id` expects, the
/// raw stdout is logged so the user can adjust the parser.
fn play(query: &str) -> String {
    eprintln!("[integration:spotify] searching for '{}'", query);
    let search = Command::new("spotify_player")
        .args(["search", query])
        .output();
    let output = match search {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let msg = format!("search failed: {}", stderr.trim());
            eprintln!("[integration:spotify] {}", msg);
            return err_body(&msg);
        }
        Err(e) => {
            let msg = format!("search spawn failed: {e}");
            eprintln!("[integration:spotify] {}", msg);
            return err_body(&msg);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let track_id = match extract_first_track_id(&stdout) {
        Some(id) => id,
        None => {
            let msg = format!(
                "could not parse a track ID from search output; first lines were: {}",
                stdout.lines().take(2).collect::<Vec<_>>().join(" | ")
            );
            eprintln!("[integration:spotify] {}", msg);
            return err_body(&msg);
        }
    };

    eprintln!("[integration:spotify] playing track {}", track_id);
    match Command::new("spotify_player")
        .args(["playback", "start", "track", "--id", &track_id])
        .status()
    {
        Ok(s) if s.success() => "{}".to_string(),
        Ok(s) => {
            let msg = format!("playback start exited with status {}", s);
            eprintln!("[integration:spotify] {}", msg);
            err_body(&msg)
        }
        Err(e) => {
            let msg = format!("playback start spawn failed: {e}");
            eprintln!("[integration:spotify] {}", msg);
            err_body(&msg)
        }
    }
}

fn control(subcommand: &str) -> String {
    if applescript_backend() {
        let verb = match subcommand {
            "play" => "play",
            "pause" => "pause",
            "next" => "next track",
            "previous" => "previous track",
            other => other,
        };
        let script = format!("tell application \"Spotify\" to {verb}");
        eprintln!("[integration:spotify] applescript: {verb}");
        match applescript::run(&script) {
            Ok(_) => "{}".to_string(),
            Err(e) => {
                let msg = format!("spotify applescript '{verb}' failed: {e}");
                eprintln!("[integration:spotify] {}", msg);
                err_body(&msg)
            }
        }
    } else {
        eprintln!("[integration:spotify] playback {}", subcommand);
        // `output()` (not `spawn()`) so we actually wait for the result and can
        // report failures back to Claude. The control subcommands are quick.
        match Command::new("spotify_player")
            .args(["playback", subcommand])
            .output()
        {
            Ok(o) if o.status.success() => "{}".to_string(),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = format!("playback {} failed: {}", subcommand, stderr.trim());
                eprintln!("[integration:spotify] {}", msg);
                err_body(&msg)
            }
            Err(e) => {
                let msg = format!("playback {} spawn failed: {}", subcommand, e);
                eprintln!("[integration:spotify] {}", msg);
                err_body(&msg)
            }
        }
    }
}

/// Parse the first track ID from `spotify_player search`'s JSON output.
/// Output shape (verified against v0.23.0):
/// ```json
/// {"tracks":[{"id":"3FijoNKG...","name":"...","artists":[...], ...}, ...]}
/// ```
/// Returns None if JSON parse fails or the tracks array is empty.
/// Caller logs raw stdout in that case so we can adjust the parser.
fn extract_first_track_id(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let tracks = json["tracks"].as_array()?;
    let first = tracks.first()?;
    first["id"].as_str().map(str::to_string)
}
