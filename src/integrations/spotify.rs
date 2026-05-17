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

/// True if the `spotify_player` binary is on PATH. We re-check on every
/// agent-loop iteration so installing the tool mid-session works without
/// restarting aegis.
pub fn is_available() -> bool {
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
    vec![
        serde_json::json!({
            "name": "spotify_play",
            "description": "Search Spotify and play the top result. Use this for ANY \
                'play X on Spotify' / 'play song X' intent when the user has Spotify \
                installed — it's dramatically faster than visually clicking through the \
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
        }),
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
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> bool {
    match name {
        "spotify_play" => {
            match input["query"].as_str() {
                Some(q) => play(q),
                None => eprintln!("[integration:spotify] spotify_play missing 'query' field"),
            }
            true
        }
        "spotify_pause" => {
            control("pause");
            true
        }
        "spotify_resume" => {
            // spotify_player calls this "play", not "resume".
            control("play");
            true
        }
        "spotify_next" => {
            control("next");
            true
        }
        "spotify_previous" => {
            control("previous");
            true
        }
        _ => false,
    }
}

/// Search for `query`, take the first track ID from the result, and start
/// playback. Two shell-outs — spotify_player's CLI doesn't (as of this
/// writing) expose a single "search and play top result" command. If the
/// search output format isn't what `extract_first_track_id` expects, the
/// raw stdout is logged so the user can adjust the parser.
fn play(query: &str) {
    eprintln!("[integration:spotify] searching for '{}'", query);
    let search = Command::new("spotify_player")
        .args(["search", query])
        .output();
    let output = match search {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "[integration:spotify] search failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return;
        }
        Err(e) => {
            eprintln!("[integration:spotify] search spawn failed: {}", e);
            return;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let track_id = match extract_first_track_id(&stdout) {
        Some(id) => id,
        None => {
            eprintln!(
                "[integration:spotify] could not parse a track ID from search output; \
                 first few lines were:\n{}",
                stdout.lines().take(5).collect::<Vec<_>>().join("\n  ")
            );
            return;
        }
    };

    eprintln!("[integration:spotify] playing track {}", track_id);
    let result = Command::new("spotify_player")
        .args(["playback", "start", "track", "--id", &track_id])
        .status();
    if let Err(e) = result {
        eprintln!("[integration:spotify] playback start spawn failed: {}", e);
    }
}

fn control(subcommand: &str) {
    eprintln!("[integration:spotify] playback {}", subcommand);
    if let Err(e) = Command::new("spotify_player")
        .args(["playback", subcommand])
        .spawn()
    {
        eprintln!(
            "[integration:spotify] playback {} spawn failed: {}",
            subcommand, e
        );
    }
}

/// Parse the first track ID from `spotify_player search`'s JSON output.
/// Output shape (verified against v0.23.0):
/// ```json
/// {"tracks":[{"id":"3FijoNKG...","name":"...","artists":[...], ...}, ...]}
/// ```
/// Returns None if JSON parse fails or the tracks array is empty —
/// caller logs raw stdout in that case so we can adjust.
fn extract_first_track_id(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let tracks = json["tracks"].as_array()?;
    let first = tracks.first()?;
    first["id"].as_str().map(str::to_string)
}
