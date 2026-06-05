/// Contract for desktop and window control: opening URLs, launching apps, and
/// window focus/inventory. These are highly desktop-environment-specific
/// (Hyprland drives them through hyprctl + gtk-launch), so each OS has its own
/// native backend selected by `target_os`. Implementors are zero-sized types.
pub trait DesktopControl {
    /// Open an http(s) URL in the user's browser.
    fn open_url(raw: &str);

    /// Launch a desktop application by name.
    fn launch_app(app: &str);

    /// Focus a window matching `target` (interpreted as a class or title).
    fn switch_to_window(target: &str);

    /// Class names of currently-open windows, used to give the agent loop
    /// "what's open right now" context. Empty on platforms without a cheap
    /// inventory primitive.
    fn list_running_apps() -> Vec<String>;
}
