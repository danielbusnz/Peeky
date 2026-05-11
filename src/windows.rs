use hyprland::data::Clients;
use hyprland::data::Workspace;
use hyprland::shared::HyprData;
use hyprland::shared::HyprDataActive;
use serde_json::Value;
use std::process::Command;

pub struct WindowRect {
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
}

pub fn active_window() -> Option<String> {
    let workspace = Workspace::get_active().ok()?;
    let clients = Clients::get().ok()?;

    clients
        .into_iter()
        .filter(|c| c.workspace.id == workspace.id)
        .min_by_key(|c| c.focus_history_id)
        .map(|c| c.title)
}

pub fn capture_window_at_coords(x: i16, y: i16) -> Option<String> {
    let clients = Clients::get().ok()?;

    clients
        .into_iter()
        .find(|c| x >= c.at.0 && x <= c.at.0 + c.size.0 && y >= c.at.1 && y <= c.at.1 + c.size.1)
        .map(|c| c.title)
}

pub fn window_dimensions(address: &str) -> Option<WindowRect> {
    let output = Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok()?;

    let clients: Value = serde_json::from_slice(&output.stdout).ok()?;

    clients
        .as_array()?
        .iter()
        .find(|c| c["address"].as_str() == Some(address))
        .map(|c| WindowRect {
            x: c["at"][0].as_i64().unwrap_or(0) as i16,
            y: c["at"][1].as_i64().unwrap_or(0) as i16,
            width: c["size"][0].as_i64().unwrap_or(0) as i16,
            height: c["size"][1].as_i64().unwrap_or(0) as i16,
        })
}

