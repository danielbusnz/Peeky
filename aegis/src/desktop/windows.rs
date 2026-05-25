//! Windows desktop control. URL opening uses the system default handler; app
//! launching and window control are not yet implemented.

use super::backend::DesktopControl;

/// Windows backend. Zero-sized; never instantiated.
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

    fn launch_app(_app: &str) {
        eprintln!("[action:launch_app] not implemented on Windows");
    }

    fn switch_to_window(_target: &str) {
        eprintln!("[action:switch_to_window] not implemented on Windows");
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
