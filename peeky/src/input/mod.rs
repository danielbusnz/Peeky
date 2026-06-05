//! OS input-event injection (click, type, key, scroll). Unlike the overlay and
//! the portable Tier-2 backends, there is no cross-platform crate for this, so
//! each OS has a native backend selected purely by `target_os`: Linux → ydotool,
//! macOS → CoreGraphics, Windows → (stub, not yet implemented).
//!
//! The `actions` module owns the public API and the serialized executor; it
//! calls the crate-internal `exec_*` functions here.

mod backend;
pub use backend::InputInjector;

#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
type Active = hyprland::Backend;
#[cfg(target_os = "macos")]
type Active = macos::Backend;
#[cfg(target_os = "windows")]
type Active = windows::Backend;

pub(crate) fn exec_click(x: i64, y: i64) {
    Active::exec_click(x, y);
}

pub(crate) fn exec_type(text: &str) {
    Active::exec_type(text);
}

pub(crate) fn exec_key(combo: &str) {
    Active::exec_key(combo);
}

pub(crate) fn exec_scroll(direction: &str, amount: u32) {
    Active::exec_scroll(direction, amount);
}

/// Startup probe; see [`InputInjector::check_available`].
pub fn check_available() {
    Active::check_available();
}
