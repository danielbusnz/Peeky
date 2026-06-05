/// Contract every mouse-position backend must satisfy.
///
/// Implementors are zero-sized types (`pub struct Backend;`). The active
/// backend is selected at compile time via a `type Active = …` alias in
/// `mod.rs`, and the public free functions delegate to `Active::*`. This
/// gives zero-cost static dispatch with no vtable and no runtime branching.
///
/// # Implementing a new backend
///
/// ```ignore
/// pub struct Backend;
/// impl MousePositionBackend for Backend {
///     fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
///         // ...
///     }
/// }
/// ```
pub trait MousePositionBackend {
    /// Returns the cursor's absolute screen position in pixels as `(x, y)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the compositor or OS cannot be queried.
    fn mouse_movement() -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>>;
}
