//! YouTube integration via the `yt-dlp` CLI.
//!
//! The flow: yt-dlp resolves a search query to the first matching video ID
//! server-side (no browser needed), then aegis opens
//! `https://www.youtube.com/watch?v=<id>` in the user's browser, which
//! YouTube auto-plays. One tool call vs. 4-6 steps of visual automation.
//!
//! Setup the user does once:
//! ```text
//! sudo pacman -S yt-dlp        # arch
//! sudo apt install yt-dlp      # debian/ubuntu
//! brew install yt-dlp          # macos (if you ever port aegis there)
//! ```

use std::process::Command;

/// True iff yt-dlp is on PATH. Hides the youtube_play tool from
/// Claude's array when false so the agent doesn't call something that
/// would fail at runtime.
pub fn is_available() -> bool {
    Command::new("which")
        .arg("yt-dlp")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Tool schemas this integration adds to Claude's tools array.
pub fn tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "name": "youtube_play",
        "description": "Search YouTube and open the top video in the user's browser. \
            Dramatically faster than navigating to youtube.com and clicking through \
            search results: yt-dlp resolves the video ID server-side and the browser \
            opens directly on the video page (autoplays). Use for 'play X on YouTube', \
            'show me X on YouTube', 'find X video', etc. \
            \
            Do NOT use for 'search YouTube for X' when the user wants to BROWSE \
            results (multiple options to choose from). Use open_url with \
            youtube.com/results?search_query=X for that. youtube_play is for the \
            single-best-result case.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query: song, video title, channel name, or combination."
                }
            },
            "required": ["query"]
        }
    })]
}

/// Returns `Some("{}")` if this integration owned the tool, `None`
/// otherwise. Fire-and-forget: the actual side effect (browser open)
/// happens in `play()` and doesn't surface a result to Claude.
pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "youtube_play" => {
            match input["query"].as_str() {
                Some(q) => play(q),
                None => eprintln!("[integration:youtube] youtube_play missing 'query' field"),
            }
            Some("{}".to_string())
        }
        _ => None,
    }
}

/// Resolve a search query to the top video ID via yt-dlp's `ytsearch1:`
/// pseudo-URL, then hand the watch URL to aegis's existing open_url
/// routing (which respects AEGIS_BROWSER + focused-window detection).
/// yt-dlp blocks ~1-3s; acceptable for one-shot voice intent.
fn play(query: &str) {
    eprintln!(
        "[integration:youtube] resolving first result for '{}'",
        query
    );
    let search = format!("ytsearch1:{}", query);
    let output = Command::new("yt-dlp")
        .args(["--get-id", "--no-warnings", &search])
        .output();

    let id = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => {
            eprintln!(
                "[integration:youtube] yt-dlp failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return;
        }
        Err(e) => {
            eprintln!("[integration:youtube] yt-dlp spawn failed: {}", e);
            return;
        }
    };

    // YouTube IDs are always 11 chars of base64url. Anything else means
    // yt-dlp returned something unexpected (multi-line, error, geo-block).
    if id.len() != 11 {
        eprintln!(
            "[integration:youtube] unexpected ID shape '{}' (len {}), aborting",
            id,
            id.len()
        );
        return;
    }

    let url = format!("https://www.youtube.com/watch?v={}", id);
    eprintln!("[integration:youtube] opening {}", url);
    crate::desktop::open_url(&url);
}
