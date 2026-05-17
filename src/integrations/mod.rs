//! Pluggable third-party app integrations (Spotify today, more later).
//!
//! Each integration is a module that exposes three free functions:
//!
//! ```ignore
//! pub fn is_available() -> bool;
//! pub fn tools() -> Vec<serde_json::Value>;
//! pub fn dispatch(name: &str, input: &serde_json::Value) -> bool;
//! ```
//!
//! No trait, no dyn dispatch — convention covers it for a hand-edited list
//! of integrations. To add a new one (e.g. Discord), create
//! `src/integrations/discord.rs` and add two lines to `all_tools` and
//! `dispatch` below. That's the entire onboarding cost.
//!
//! An integration's `is_available()` returning false hides its tools
//! entirely from Claude's tools array — so Claude can't try to call a
//! Spotify tool if spotify_player isn't installed. No "tool failed at
//! runtime" surprises.

pub mod spotify;

/// Tool schemas to inject into Claude's tools array. Only tools from
/// integrations whose `is_available()` returns true are included.
pub fn all_tools() -> Vec<serde_json::Value> {
    let mut tools: Vec<serde_json::Value> = vec![];
    if spotify::is_available() {
        tools.extend(spotify::tools());
    }
    tools
}

/// Try to dispatch a tool call to whichever integration owns it. Returns
/// true if some integration handled the call (regardless of whether the
/// underlying command succeeded), false if no integration recognized the
/// tool name — caller logs the unknown name.
pub fn dispatch(name: &str, input: &serde_json::Value) -> bool {
    if spotify::dispatch(name, input) {
        return true;
    }
    false
}
