//! Cursor overlay window. The implementation is selected at compile time
//! by a Cargo feature so the same callers work regardless of platform.

#[cfg(feature = "hyprland")]
mod hyprland;
#[cfg(feature = "hyprland")]
pub use hyprland::*;

#[cfg(feature = "winit-window")]
mod winit;
#[cfg(feature = "winit-window")]
pub use winit::*;
