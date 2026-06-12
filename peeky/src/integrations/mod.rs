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
pub mod apps;
pub mod calendar;
pub mod clipboard;
pub mod contacts;
pub mod facetime;
pub mod finder;
pub mod github;
pub mod gmail;
pub mod health;
pub mod keynote;
pub mod mail;
pub mod maps;
pub mod messages;
pub mod music;
pub mod notes;
pub mod photos;
pub mod reminders;
pub mod safari;
pub mod shortcuts;
pub mod spotify;
pub mod spotlight;
pub mod system;
pub mod type_text;
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
    if safari::is_available() {
        tools.extend(safari::tools());
    }
    if system::is_available() {
        tools.extend(system::tools());
    }
    if reminders::is_available() {
        tools.extend(reminders::tools());
    }
    if notes::is_available() {
        tools.extend(notes::tools());
    }
    if finder::is_available() {
        tools.extend(finder::tools());
    }
    if calendar::is_available() {
        tools.extend(calendar::tools());
    }
    if apps::is_available() {
        tools.extend(apps::tools());
    }
    if shortcuts::is_available() {
        tools.extend(shortcuts::tools());
    }
    if messages::is_available() {
        tools.extend(messages::tools());
    }
    if contacts::is_available() {
        tools.extend(contacts::tools());
    }
    if music::is_available() {
        tools.extend(music::tools());
    }
    if mail::is_available() {
        tools.extend(mail::tools());
    }
    if photos::is_available() {
        tools.extend(photos::tools());
    }
    if keynote::is_available() {
        tools.extend(keynote::tools());
    }
    if clipboard::is_available() {
        tools.extend(clipboard::tools());
    }
    if spotlight::is_available() {
        tools.extend(spotlight::tools());
    }
    if facetime::is_available() {
        tools.extend(facetime::tools());
    }
    if maps::is_available() {
        tools.extend(maps::tools());
    }
    if type_text::is_available() {
        tools.extend(type_text::tools());
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
    if let Some(result) = safari::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = system::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = reminders::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = notes::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = finder::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = calendar::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = apps::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = shortcuts::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = messages::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = contacts::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = music::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = mail::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = photos::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = keynote::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = clipboard::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = spotlight::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = facetime::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = maps::dispatch(name, input) {
        return Some(result);
    }
    if let Some(result) = type_text::dispatch(name, input) {
        return Some(result);
    }
    None
}
