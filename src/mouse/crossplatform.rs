use mouse_position::mouse_position::Mouse;

/// Query the global cursor position. Works on Windows, macOS, and X11.
/// Wayland is intentionally unsupported by the underlying crate — use
/// the compositor-specific impl (`mouse/hyprland.rs`) on Wayland.
pub fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error>> {
    match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => Ok((x as i64, y as i64)),
        Mouse::Error => Err("could not query cursor position".into()),
    }
}
