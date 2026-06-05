use super::backend::MousePositionBackend;
use hyprland::data::CursorPosition;
use hyprland::shared::HyprData;

/// Hyprland IPC backend. Zero-sized; never instantiated.
pub struct Backend;

impl MousePositionBackend for Backend {
    /// Cursor position in absolute screen pixels via Hyprland IPC.
    ///
    /// # Errors
    ///
    /// Propagates any `hyprland::Error` from the IPC socket query.
    fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
        let pos = CursorPosition::get()?;
        Ok((pos.x, pos.y))
    }
}
