//! Hyprland desktop control: hyprctl for window focus/inventory, gtk-launch
//! for apps, and Chromium-family-aware URL routing.

use std::process::Command;
use std::thread;
use std::time::Duration;

use super::backend::DesktopControl;

/// Hyprland backend. Zero-sized; never instantiated.
pub struct Backend;

impl DesktopControl for Backend {
    /// Open a URL in the user's currently-focused browser when possible,
    /// falling back to xdg-open. Priority:
    ///   1. `AEGIS_BROWSER` env var: force a specific binary.
    ///   2. Hyprland's currently-focused window, if it's a Chromium-family
    ///      browser (Chrome, Brave, Chromium, Edge, Vivaldi). Chromium-family
    ///      can be invoked directly without D-Bus session issues.
    ///   3. xdg-open: uses the system default browser. Necessary for Firefox
    ///      since direct `firefox <url>` calls hang on D-Bus when aegis isn't
    ///      in the user session.
    fn open_url(raw: &str) {
        let parsed = match url::Url::parse(raw) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("[action:open_url] rejecting '{}': {}", raw, e);
                return;
            }
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            eprintln!(
                "[action:open_url] rejecting non-http scheme '{}'",
                parsed.scheme()
            );
            return;
        }

        eprintln!("[action:open_url] opening {}", raw);

        if let Ok(forced) = std::env::var("AEGIS_BROWSER") {
            eprintln!("[action:open_url] AEGIS_BROWSER override → {}", forced);
            if let Err(e) = Command::new(&forced).arg(raw).spawn() {
                eprintln!("[action:open_url] AEGIS_BROWSER spawn failed: {}", e);
            }
            raise_likely_browser();
            return;
        }

        if let Some(bin) = focused_browser_binary() {
            eprintln!(
                "[action:open_url] focused window is {} → routing there",
                bin
            );
            if let Err(e) = Command::new(&bin).arg(raw).spawn() {
                eprintln!(
                    "[action:open_url] direct browser spawn failed ({}), falling back to xdg-open: {}",
                    bin, e
                );
                let _ = open::that_detached(raw);
            }
            raise_likely_browser();
            return;
        }

        eprintln!("[action:open_url] no Chromium-family browser focused → xdg-open (default)");
        if let Err(e) = open::that_detached(raw) {
            eprintln!("[action:open_url] xdg-open failed: {}", e);
            return;
        }

        raise_likely_browser();
    }

    /// Tries gtk-launch first (handles .desktop entries like "spotify" →
    /// spotify.desktop), falls back to spawning the binary directly via shell
    /// `||`. setsid -f fully detaches so the app survives if aegis exits.
    fn launch_app(app: &str) {
        eprintln!("[action:launch_app] launching '{}'", app);
        let escaped = shell_single_quote(app);
        let cmd = format!("gtk-launch {esc} 2>/dev/null || exec {esc}", esc = escaped);
        if let Err(e) = Command::new("setsid")
            .args(["-f", "sh", "-c", &cmd])
            .spawn()
        {
            eprintln!("[action:launch_app] spawn failed: {}", e);
        }
    }

    /// Focuses a window matching `target` as a class first, then as a title
    /// substring 150ms later. Claude's `target` is whatever the user said,
    /// which is ambiguous between class names ("firefox") and title text
    /// ("Inbox"), so we try both. Non-matches fail silently in hyprctl.
    fn switch_to_window(target: &str) {
        eprintln!("[action:switch_to_window] focusing '{}'", target);
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("class:{}", target)])
            .spawn();
        let target = target.to_string();
        thread::spawn(move || {
            // > hyprctl dispatch round-trip (~30ms) so the class attempt
            // resolves first; < perceptual instant.
            thread::sleep(Duration::from_millis(150));
            let _ = Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &format!("title:{}", target)])
                .spawn();
        });
    }

    /// Distinct window classes of all currently-mapped Hyprland clients.
    /// Empty Vec on any failure.
    fn list_running_apps() -> Vec<String> {
        let Ok(output) = Command::new("hyprctl").args(["clients", "-j"]).output() else {
            return vec![];
        };
        if !output.status.success() {
            return vec![];
        }
        let Ok(arr) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
            return vec![];
        };
        let Some(clients) = arr.as_array() else {
            return vec![];
        };
        let mut classes: Vec<String> = clients
            .iter()
            .filter_map(|c| c["class"].as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        classes.sort();
        classes.dedup();
        classes
    }
}

/// Query Hyprland for the currently-focused window's class. If that class
/// maps to a Chromium-family browser AND the corresponding binary exists
/// on PATH, return the binary name. Returns None for Firefox (intentionally,
/// since direct calls hang on D-Bus) and for non-browser windows.
fn focused_browser_binary() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let window: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let class = window["class"].as_str()?.to_lowercase();

    let candidates: &[&str] = match class.as_str() {
        // Firefox-family deliberately not routed directly. Defer to xdg-open
        // so it goes through the session's lock-file + D-Bus handshake.
        "firefox" | "firefox-esr" | "librewolf" | "waterfox" | "zen" => return None,
        "chromium" | "chromium-browser" => &["chromium"],
        "google-chrome" | "google-chrome-stable" | "chrome" => {
            &["google-chrome-stable", "google-chrome", "chrome"]
        }
        "brave-browser" | "brave-browser-stable" | "brave" => &["brave-browser", "brave"],
        "vivaldi-stable" | "vivaldi" => &["vivaldi-stable", "vivaldi"],
        "microsoft-edge" | "microsoft-edge-stable" | "msedge" => {
            &["microsoft-edge-stable", "microsoft-edge", "msedge"]
        }
        _ => return None,
    };

    candidates
        .iter()
        .find(|bin| binary_on_path(bin))
        .map(|s| s.to_string())
}

/// True iff `which <bin>` succeeds. Output suppressed because misses are
/// expected (caller probes several candidates).
fn binary_on_path(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Works around Hyprland's XDG-activation focus-steal block by dispatching
/// focuswindow at every common browser class after the new window is
/// likely to exist. Misses no-op.
fn raise_likely_browser() {
    thread::spawn(|| {
        // Below ~300ms the focuswindow dispatch can race the browser's
        // window creation, leaving Hyprland with no matching client.
        thread::sleep(Duration::from_millis(300));
        for class in &[
            "firefox",
            "Chromium",
            "Brave-browser",
            "Google-chrome",
            "chromium",
        ] {
            let _ = Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &format!("class:{}", class)])
                .spawn();
        }
    });
}

/// Single-quote escape for shell -c. Replaces ' with '\'' (close, escape, reopen).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
