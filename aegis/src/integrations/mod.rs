//! Pluggable third-party app integrations (Spotify today, more later).
//!
//! Each integration is a module that exposes three free functions:
//!
//! ```ignore
//! pub fn is_available() -> bool;
//! pub fn tools() -> Vec<serde_json::Value>;
//! pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String>;
//! ```
//!
//! No trait, no dyn dispatch. Convention covers it for a hand-edited
//! list of integrations. To add a new one (e.g. Discord), create
//! `src/integrations/discord.rs` and add two lines to `all_tools` and
//! `dispatch` below. That's the entire onboarding cost.
//!
//! `dispatch` returns `None` if this integration does not own the named tool,
//! or `Some(result_json)` if it does. Fire-and-forget tools return
//! `Some("{}")`.  Data-returning tools (search, read, count) return a
//! meaningful JSON string that the agent loop surfaces to Claude as the
//! tool_result content.
//!
//! An integration's `is_available()` returning false hides its tools
//! entirely from Claude's tools array, so Claude can't try to call a
//! Spotify tool if spotify_player isn't installed. No "tool failed at
//! runtime" surprises.

pub mod applescript;
pub mod github;
pub mod gmail;
pub mod health;
pub mod spotify;
pub mod youtube;

/// Tool schemas to inject into Claude's tools array. Only tools from
/// integrations whose `is_available()` returns true are included.
pub fn all_tools() -> Vec<serde_json::Value> {
    let mut tools: Vec<serde_json::Value> = vec![];
    if spotify::is_available() {
        tools.extend(spotify::tools());
    }
    if youtube::is_available() {
        tools.extend(youtube::tools());
    }
    if gmail::is_available() {
        tools.extend(gmail::tools());
    }
    if github::is_available() {
        tools.extend(github::tools());
    }
    tools
}

/// Try to dispatch a tool call to whichever integration owns it. Returns
/// `None` if no integration recognized the tool name (caller logs the
/// unknown name), or `Some(result_json)` if an integration handled it.
pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    if let Some(result) = spotify::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = youtube::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = gmail::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = github::dispatch(name, input) {
        return Some(result);
    }
    None
}
