//! Screen + monitor geometry capture. Backend selected at compile time by
//! target OS.
//!
//! Callers use the free functions directly; the backend is an implementation
//! detail. `pick_declared_resolution` is backend-independent and lives in
//! `shared.rs`.

mod backend;
mod shared;
pub use backend::ScreenshotBackend;
// Re-exported for callers; not every binary that links this module uses it
// (e.g. demo_win), so the unused-in-some-bins lint is expected.
#[allow(unused_imports)]
pub use shared::pick_declared_resolution;

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod crossplatform;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod hyprland;

// Linux uses the native grim backend; everything else (and Linux under the
// no-default-features build) uses the portable xcap backend. The two
// cfgs are mutually exclusive and exhaustive, so exactly one Active is defined.
#[cfg(all(target_os = "linux", feature = "hyprland"))]
type Active = hyprland::Backend;
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
type Active = crossplatform::Backend;

/// Geometry `(x, y, width, height)` of the monitor a capture should target.
///
/// # Errors
///
/// Propagates any error from the active backend.
pub fn active_workspace_geometry()
-> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
    Active::active_workspace_geometry()
}

/// Capture a region, resize to `(target_w, target_h)`, encode JPEG q85, base64.
///
/// # Errors
///
/// Propagates any error from the active backend.
pub fn capture_resized_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    target_w: u32,
    target_h: u32,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Active::capture_resized_for_claude(x, y, width, height, target_w, target_h)
}

/// Capture a region and return base64 plus captured dimensions. Cross-platform
/// backend only (the `demo_win` binary); grim has no equivalent.
///
/// # Errors
///
/// Propagates any error from the `xcap` backend.
#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
#[allow(dead_code)]
pub fn capture_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(String, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
    crossplatform::Backend::capture_for_claude(x, y, width, height)
}
