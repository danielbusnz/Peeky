/// Contract every push-to-talk hotkey backend must satisfy.
///
/// Implementors are zero-sized types (`pub struct Backend;`). The active
/// backend is selected at compile time via a `type Active = …` alias in
/// `mod.rs`, and the public free functions delegate to `Active::*`. This
/// gives zero-cost static dispatch with no vtable and no runtime branching.
pub trait HotkeyBackend {
    /// Start listening for the push-to-talk key. On macOS this must run on
    /// the main thread (the `global-hotkey` manager requires it).
    ///
    /// # Errors
    ///
    /// Returns an error if the listener or manager cannot be set up.
    fn init() -> std::io::Result<()>;

    /// True while the hotkey is held. Cheap; reads a relaxed atomic.
    fn is_recording() -> bool;

    /// Block the calling thread until the hotkey is pressed.
    fn wait_for_press();

    /// Register a callback fired on press. The signal backend invokes it from
    /// its listener thread; the polling backend treats callbacks as unused
    /// (the overlay reads `is_recording` directly instead). Boxed so the
    /// method is non-generic and the one-time allocation happens at
    /// registration, not on each invocation.
    ///
    /// `allow(dead_code)`: only the signal backend's build calls this.
    #[allow(dead_code)]
    fn on_press(f: Box<dyn Fn() + Send + Sync + 'static>);

    /// Register a callback fired on release. See [`on_press`](Self::on_press).
    #[allow(dead_code)]
    fn on_release(f: Box<dyn Fn() + Send + Sync + 'static>);

    /// Drain pending hotkey events into the recording state. Required by
    /// backends whose events arrive on a queue (e.g. `global-hotkey`); a
    /// no-op for backends with an independent listener thread (signals).
    /// Call once per iteration of the main event loop.
    ///
    /// `allow(dead_code)`: only the polling backend's build calls this.
    #[allow(dead_code)]
    fn poll() {}
}
