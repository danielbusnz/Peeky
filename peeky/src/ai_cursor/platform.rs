//! Platform-specific window configuration abstraction.
//!
//! Provides a unified interface for platform-specific window setup,
//! delegating to macos.rs on macOS and providing defaults elsewhere.

use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

#[cfg(not(target_os = "macos"))]
use winit::window::Fullscreen;

#[cfg(target_os = "macos")]
use super::macos;

/// Apply platform-specific window attributes.
/// On macOS: returns attrs as-is (fullscreen kills transparency).
/// On other platforms: applies borderless fullscreen.
pub fn apply_window_attrs(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "macos")]
    {
        // macOS: skip fullscreen, we size the window manually in post_window_create
        attrs
    }
    #[cfg(not(target_os = "macos"))]
    {
        attrs.with_fullscreen(Some(Fullscreen::Borderless(None)))
    }
}

/// Called after window creation to apply platform-specific configuration.
/// On macOS: sizes window to fill screen without fullscreen mode.
/// On other platforms: no-op (fullscreen already set via attrs).
pub fn post_window_create(event_loop: &ActiveEventLoop, window: &Window) {
    #[cfg(target_os = "macos")]
    macos::configure_window_size(event_loop, window);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (event_loop, window); // suppress unused warnings
    }
}

/// Scale cursor position for the platform's coordinate system.
/// On macOS: scales logical points to physical pixels for Retina displays.
/// On other platforms: returns position unchanged.
pub fn scale_cursor_position(window: &Window, pos: (f64, f64)) -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        macos::scale_cursor_position(window, pos)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window; // suppress unused warning
        pos
    }
}

/// Configure platform-specific transparency settings.
/// On macOS: sets NSWindow to non-opaque with clear background.
/// On other platforms: no-op.
pub fn configure_transparency(window: &Arc<Window>) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: configure_window_transparency uses raw Objective-C messaging
        // to configure NSWindow properties for transparency.
        unsafe { macos::configure_window_transparency(window) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window; // suppress unused warning
    }
}
