//! Cursor overlay window. The implementation is selected at compile time by
//! target OS (Linux/Hyprland via GTK layer-shell, macOS/Windows via winit), so
//! the same callers work regardless of platform.
//!
//! NOTE: macOS currently rides the winit path (winit window + wgpu + NSWindow
//! tweaks in macos.rs). A native AppKit/NSPanel overlay is the intended end
//! state but is deferred: it can only be built and tested on macOS.

/// Visual state of the overlay cursor. The painter reads this to choose
/// what to render; the orchestrator writes it as voice turns progress.
///
/// `Idle` is only constructed on the Linux/Hyprland path; the winit path
/// matches on it but never assigns it. The `allow(dead_code)` is for the
/// latter so the variant stays in the public enum.
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

// Shared types and utilities used by the winit overlay implementation.
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod common;

// macOS-specific window configuration (only compiled on macOS)
#[cfg(target_os = "macos")]
mod macos;

// Platform abstraction layer for window configuration
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod platform;

// Cross-platform renderer abstraction (softbuffer on non-macOS, wgpu on macOS)
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod renderer;

// Linux/Hyprland: GTK layer-shell overlay.
#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod hyprland;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
pub use hyprland::*;

// macOS/Windows (and Linux under the dev override): winit overlay.
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod winit;
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
pub use winit::*;
