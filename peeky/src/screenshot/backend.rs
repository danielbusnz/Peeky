/// Contract every screenshot backend must satisfy.
///
/// Implementors are zero-sized types (`pub struct Backend;`). The active
/// backend is selected at compile time via a `type Active = …` alias in
/// `mod.rs`, and the public free functions delegate to `Active::*`. This
/// gives zero-cost static dispatch with no vtable and no runtime branching.
///
/// `pick_declared_resolution` is intentionally NOT part of the contract: it
/// is pure math with no backend dependency and lives in `shared.rs`.
pub trait ScreenshotBackend {
    /// Geometry `(x, y, width, height)` of the monitor a capture should
    /// target. On Hyprland this is the monitor showing the active workspace;
    /// elsewhere it is the primary monitor.
    ///
    /// # Errors
    ///
    /// Returns an error if the compositor or OS cannot report monitor layout.
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>>;

    /// Capture a screen region, resize to `(target_w, target_h)` with bilinear
    /// filtering, encode as JPEG q85, and return base64. One decode and one
    /// encode end to end. This is the hot path the agent loop runs each turn.
    ///
    /// # Errors
    ///
    /// Returns an error if capture, resize, or encode fails.
    fn capture_resized_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}
