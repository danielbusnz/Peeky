//! One-shot conversation handoff from Claude Code.
//!
//! When Aegis is spawned via the Claude Code `/aegis` command, that command
//! writes a short summary of what the user was working on to
//! `<config>/aegis/handoff.md`. We read it once at startup and delete it, so
//! the context is tied to that one spawn and never goes stale on later
//! launches. The chat path injects it as a cached system block so a voice turn
//! like "what were we just doing?" has the context.

use std::sync::OnceLock;

static HANDOFF: OnceLock<Option<String>> = OnceLock::new();

/// Read and consume the handoff file. Call once at startup, before the chat
/// path can run. Best effort: a missing or empty file just means no handoff.
pub fn init() {
    let loaded = read_and_delete();
    match &loaded {
        Some(s) => eprintln!("[handoff] loaded {} chars of Claude Code context", s.len()),
        None => eprintln!("[handoff] no handoff file; spawned without conversation context"),
    }
    let _ = HANDOFF.set(loaded);
}

/// The handoff text captured at startup, if any.
pub fn get() -> Option<&'static str> {
    HANDOFF.get().and_then(|o| o.as_deref())
}

fn read_and_delete() -> Option<String> {
    let path = dirs::config_dir()?.join("aegis").join("handoff.md");
    let content = std::fs::read_to_string(&path).ok()?;
    // Consume it: this context belongs to the spawn that wrote it.
    let _ = std::fs::remove_file(&path);
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
