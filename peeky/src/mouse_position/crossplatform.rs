use super::backend::MousePositionBackend;
use mouse_position::mouse_position::Mouse;

/// Cross-platform backend (Windows, macOS, X11). Zero-sized; never instantiated.
///
/// Wayland is intentionally unsupported by the underlying `mouse_position`
/// crate. Use the Hyprland backend for Wayland compositors.
pub struct Backend;

impl MousePositionBackend for Backend {
    /// Cursor position in absolute screen pixels.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS cannot be queried (the underlying crate
    /// surfaces this as a sentinel variant rather than a typed error).
    fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
        match Mouse::get_mouse_position() {
            Mouse::Position { x, y } => Ok((x as i64, y as i64)),
            Mouse::Error => Err("could not query cursor position".into()),
        }
    }
}
