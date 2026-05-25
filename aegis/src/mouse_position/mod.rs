//! Mouse position polling. Backend selected at compile time by target OS.
//!
//! Callers use the free functions directly; the backend is an implementation
//! detail. Adding a new backend requires only:
//!   1. A new file implementing `MousePositionBackend`.
//!   2. A `#[cfg(...)] mod new_backend;` + type alias arm below.
//!   3. No changes to any call site.

mod backend;
pub use backend::MousePositionBackend;

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod crossplatform;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod hyprland;

// Linux uses the native Hyprland backend; everything else (and Linux under the
// no-default-features build) uses the portable backend. The two cfgs are
// mutually exclusive and exhaustive, so exactly one Active is always defined.
#[cfg(all(target_os = "linux", feature = "hyprland"))]
type Active = hyprland::Backend;
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
type Active = crossplatform::Backend;

/// Returns the cursor's absolute screen position as `(x, y)` in pixels.
///
/// # Errors
///
/// Propagates any error from the active backend.
///
/// # Example
///
/// ```no_run
/// let (x, y) = aegis::mouse_position::mouse_movement().unwrap();
/// println!("cursor at ({x}, {y})");
/// ```
pub fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
    Active::mouse_movement()
}
