use hyprland::data::Clients;
use hyprland::data::CursorPosition;
use hyprland::data::Workspace;
use hyprland::shared::HyprData;
use hyprland::shared::HyprDataActive;

pub fn mouse_movement() -> hyprland::Result<(i64, i64)> {
    let pos = CursorPosition::get()?;

    Ok((pos.x, pos.y))
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
