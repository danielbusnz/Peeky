//! Desktop and window control: opening URLs, launching apps, focusing windows,
//! and listing open windows. Backend selected by `target_os` (Linux → Hyprland,
//! macOS/Windows → native). Unlike the overlay, this is keyed purely on
//! `target_os`, not the `hyprland` feature: it reflects the actual running
//! desktop environment, and on Linux that is always Hyprland.

mod backend;
pub use backend::DesktopControl;

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

/// Open an http(s) URL in the user's browser.
pub fn open_url(raw: &str) {
    Active::open_url(raw);
}

/// Launch a desktop application by name.
pub fn launch_app(app: &str) {
    Active::launch_app(app);
}

/// Focus a window matching `target` (class or title).
pub fn switch_to_window(target: &str) {
    Active::switch_to_window(target);
}

/// Class names of currently-open windows, for agent-loop context.
pub fn list_running_apps() -> Vec<String> {
    Active::list_running_apps()
}
