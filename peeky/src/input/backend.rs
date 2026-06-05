/// Contract every input-injection backend must satisfy. Each method
/// synthesizes a real OS input event. Implementors are zero-sized types
/// (`pub struct Backend;`); the active one is selected by `target_os` in
/// `mod.rs` and the crate-internal free functions delegate to it.
pub trait InputInjector {
    /// Move the system cursor to `(x, y)` and fire a left button down+up.
    fn exec_click(x: i64, y: i64);

    /// Type `text` into the focused field. A trailing `\n` submits (fires
    /// Enter after typing).
    fn exec_type(text: &str);

    /// Press a key or combo in human syntax (`"Return"`, `"ctrl+a"`).
    fn exec_key(combo: &str);

    /// Scroll `amount` wheel-clicks in `direction`, approximated with
    /// repeated arrow-key presses.
    fn exec_scroll(direction: &str, amount: u32);

    /// Startup probe: log whether real input injection is available on this
    /// platform, with setup guidance if not. Never fails. Pointing, opening
    /// URLs, and launching apps still work without it.
    fn check_available();
}
