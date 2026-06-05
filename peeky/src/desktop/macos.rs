//! macOS desktop control. URL opening and app launching go through the system
//! `open` mechanism; window focus/inventory are not yet implemented.

use std::process::Command;

use super::backend::DesktopControl;

/// macOS backend. Zero-sized; never instantiated.
pub struct Backend;

impl DesktopControl for Backend {
    fn open_url(raw: &str) {
        if !is_http(raw) {
            eprintln!("[action:open_url] rejecting non-http url '{}'", raw);
            return;
        }
        if let Err(e) = open::that_detached(raw) {
            eprintln!("[action:open_url] open failed: {}", e);
        }
    }

    fn launch_app(app: &str) {
        // `open -a <app>` launches a macOS .app by display name.
        if let Err(e) = Command::new("open").args(["-a", app]).spawn() {
            eprintln!("[action:launch_app] open -a failed: {}", e);
        }
    }

    fn switch_to_window(_target: &str) {
        eprintln!("[action:switch_to_window] not implemented on macOS");
    }

    fn list_running_apps() -> Vec<String> {
        Vec::new()
    }
}

fn is_http(raw: &str) -> bool {
    url::Url::parse(raw)
        .map(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or(false)
}
