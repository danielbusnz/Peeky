//! Cursor overlay window. The implementation is selected at compile time
//! by a Cargo feature so the same callers work regardless of platform.

/// Visual state of the overlay cursor. The painter reads this to choose
/// what to render; the orchestrator writes it as voice turns progress.
///
/// `Idle` is only constructed from the hyprland feature path; the winit
/// build path matches on it but never assigns it. The `allow(dead_code)`
/// is for the latter so the variant stays in the public enum.
#[derive(Debug)]
#[allow(dead_code)]
pub enum CursorState {
    /// Default. Cursor follows the system mouse, no soundwave.
    Idle,
    /// Hotkey held, mic capturing. Cursor renders a live soundwave.
    Listening,
    /// Hotkey released, waiting for Claude's first response or first
    /// PCM chunk. Cursor renders a loading animation.
    Loading,
}

#[cfg(feature = "hyprland")]
mod hyprland;
#[cfg(feature = "hyprland")]
pub use hyprland::*;

#[cfg(feature = "winit-window")]
mod winit;
#[cfg(feature = "winit-window")]
pub use winit::*;
