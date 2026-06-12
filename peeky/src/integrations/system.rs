//! System control via AppleScript (`osascript`) and official Apple CLIs.
//! macOS only. Volume, mute, dark mode, sleep, screen saver, wallpaper,
//! notifications, keep-awake (`caffeinate`), and Wi-Fi power (`networksetup`).
//! These are system settings, not an app, so there is no install, auth, or
//! window involved.
//!
//! Screen brightness is deliberately absent: macOS exposes no AppleScript for
//! it (it needs key-code simulation or a separate CLI), so it is not a quick add
//! and does not belong in this module.

use std::process::Command;

use super::applescript;

/// True on macOS, where `osascript` and the Apple CLIs are always present.
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `system_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "system_set_volume",
            "description": "Set the system output volume, 0 (silent) to 100 (max). \
                Use for 'turn it up', 'turn it down', 'set volume to 50'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "level": { "type": "integer", "description": "Volume from 0 to 100." }
                },
                "required": ["level"]
            }
        }),
        serde_json::json!({
            "name": "system_mute",
            "description": "Mute system audio output.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_unmute",
            "description": "Unmute system audio output.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_dark_mode",
            "description": "Switch the system appearance. Use for 'turn on dark \
                mode', 'switch to light mode', 'toggle dark mode'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["on", "off", "toggle"],
                        "description": "'on' for dark, 'off' for light, 'toggle' to flip."
                    }
                },
                "required": ["mode"]
            }
        }),
        serde_json::json!({
            "name": "system_sleep",
            "description": "Put the Mac to sleep. Use for 'go to sleep', \
                'sleep my mac'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_screensaver",
            "description": "Start the screen saver, which locks the screen when \
                password-on-wake is enabled. Use for 'lock my screen', \
                'start the screensaver'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_set_wallpaper",
            "description": "Set the desktop wallpaper on every display. Use for \
                'change my wallpaper to X'. Find the image with spotlight_search \
                first if the user names it loosely.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute POSIX path to an image file."
                    }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "system_notify",
            "description": "Post a macOS notification banner. Use when the user \
                asks to be visually notified of something that just completed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "The notification title." },
                    "message": { "type": "string", "description": "The notification body." }
                },
                "required": ["title", "message"]
            }
        }),
        serde_json::json!({
            "name": "system_keep_awake",
            "description": "Keep the Mac and display awake for a number of \
                minutes (caffeinate). Use for 'keep my mac awake for an hour', \
                'don't let the screen sleep'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "minutes": {
                        "type": "integer",
                        "description": "How long to stay awake, 1 to 1440 minutes."
                    }
                },
                "required": ["minutes"]
            }
        }),
        serde_json::json!({
            "name": "system_allow_sleep",
            "description": "Cancel a previous keep-awake and let the Mac sleep \
                normally again. Use for 'let my mac sleep again'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "system_wifi",
            "description": "Turn Wi-Fi on or off. Use for 'turn off wifi', \
                'turn wifi back on'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "on": { "type": "boolean", "description": "true for on, false for off." }
                },
                "required": ["on"]
            }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "system_set_volume" => Some(match input["level"].as_i64() {
            Some(level) => set_volume(level),
            None => err_body("system_set_volume missing integer 'level' field"),
        }),
        "system_mute" => Some(set_muted(true)),
        "system_unmute" => Some(set_muted(false)),
        "system_dark_mode" => Some(match input["mode"].as_str() {
            Some(mode) => dark_mode(mode),
            None => err_body("system_dark_mode missing 'mode' field"),
        }),
        "system_sleep" => Some(sleep()),
        "system_screensaver" => Some(screensaver()),
        "system_set_wallpaper" => Some(match input["path"].as_str() {
            Some(path) => set_wallpaper(path),
            None => err_body("system_set_wallpaper missing 'path' field"),
        }),
        "system_notify" => {
            let title = match input["title"].as_str() {
                Some(t) => t,
                None => return Some(err_body("system_notify missing 'title' field")),
            };
            let message = match input["message"].as_str() {
                Some(m) => m,
                None => return Some(err_body("system_notify missing 'message' field")),
            };
            Some(notify(title, message))
        }
        "system_keep_awake" => Some(match input["minutes"].as_i64() {
            Some(minutes) => keep_awake(minutes),
            None => err_body("system_keep_awake missing integer 'minutes' field"),
        }),
        "system_allow_sleep" => Some(allow_sleep()),
        "system_wifi" => Some(match input["on"].as_bool() {
            Some(on) => wifi(on),
            None => err_body("system_wifi missing boolean 'on' field"),
        }),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so failures reach Claude as tool_result
/// content, matching the shape the other integrations use.
fn err_body(msg: &str) -> String {
    format!(
        r#"{{"error":{}}}"#,
        serde_json::Value::String(msg.to_string())
    )
}

/// Set output volume, clamping the model's level into AppleScript's 0..=100.
fn set_volume(level: i64) -> String {
    let script = format!("set volume output volume {}", level.clamp(0, 100));
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system set_volume failed: {e}")),
    }
}

/// `set volume with output muted` / `without output muted`.
fn set_muted(muted: bool) -> String {
    let clause = if muted { "with" } else { "without" };
    let script = format!("set volume {clause} output muted");
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system set_muted failed: {e}")),
    }
}

/// Flip the appearance via System Events' appearance preferences. The mode is
/// validated against the schema enum, so anything else is a model error.
fn dark_mode(mode: &str) -> String {
    let value = match mode {
        "on" => "true",
        "off" => "false",
        "toggle" => "not dark mode",
        other => return err_body(&format!("system_dark_mode unknown mode '{other}'")),
    };
    let script = format!(
        "tell application \"System Events\" to tell appearance preferences to set dark mode to {value}"
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_dark_mode failed: {e}")),
    }
}

/// Fire-and-forget: put the machine to sleep. The reply is spoken before the
/// turn ends, so the lid stays metaphorically open long enough to hear it.
fn sleep() -> String {
    match applescript::run("tell application \"System Events\" to sleep") {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_sleep failed: {e}")),
    }
}

/// Start the current screen saver. With password-on-wake enabled this is the
/// scriptable way to lock the screen (macOS has no direct lock command).
fn screensaver() -> String {
    match applescript::run("tell application \"System Events\" to start current screen saver") {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_screensaver failed: {e}")),
    }
}

/// Point every desktop's picture at the file. System Events accepts a plain
/// POSIX path string here, no `POSIX file` coercion needed.
fn set_wallpaper(path: &str) -> String {
    let script = format!(
        "tell application \"System Events\" to set picture of every desktop to \"{}\"",
        applescript::escape(path)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_set_wallpaper failed: {e}")),
    }
}

/// Post a notification banner via Standard Additions' `display notification`.
fn notify(title: &str, message: &str) -> String {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript::escape(message),
        applescript::escape(title)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_notify failed: {e}")),
    }
}

/// Spawn `caffeinate -d -t <secs>` through `sh ... &` so the shell exits
/// immediately and caffeinate is reparented, leaving no zombie for peeky to
/// reap. `-d` also keeps the display awake, which is what "keep my mac awake"
/// means to a person looking at it.
fn keep_awake(minutes: i64) -> String {
    let seconds = minutes.clamp(1, 1440) * 60;
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("caffeinate -d -t {seconds} >/dev/null 2>&1 &"))
        .status();
    match status {
        Ok(s) if s.success() => "{}".to_string(),
        Ok(s) => err_body(&format!("system_keep_awake failed: sh exited {s}")),
        Err(e) => err_body(&format!("system_keep_awake spawn failed: {e}")),
    }
}

/// Kill any running caffeinate. `killall` exiting nonzero just means none was
/// running, which is the state the user asked for, so it is not an error.
fn allow_sleep() -> String {
    match Command::new("killall").arg("caffeinate").status() {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("system_allow_sleep spawn failed: {e}")),
    }
}

/// Toggle Wi-Fi power: find the Wi-Fi device with `networksetup
/// -listallhardwareports`, then `-setairportpower <dev> on|off`. Neither
/// command needs sudo.
fn wifi(on: bool) -> String {
    let ports = match Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => {
            return err_body(&format!(
                "system_wifi listallhardwareports failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Err(e) => return err_body(&format!("system_wifi spawn failed: {e}")),
    };
    let device = match wifi_device(&ports) {
        Some(dev) => dev.to_string(),
        None => return err_body("system_wifi found no Wi-Fi hardware port"),
    };
    let state = if on { "on" } else { "off" };
    match Command::new("networksetup")
        .args(["-setairportpower", &device, state])
        .output()
    {
        Ok(out) if out.status.success() => "{}".to_string(),
        Ok(out) => err_body(&format!(
            "system_wifi setairportpower failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => err_body(&format!("system_wifi spawn failed: {e}")),
    }
}

/// Pull the device name (e.g. `en0`) out of `-listallhardwareports` output:
/// the `Device:` line that follows the `Hardware Port: Wi-Fi` line. Split out
/// from `wifi` so it can be unit-tested without running networksetup.
fn wifi_device(ports: &str) -> Option<&str> {
    let mut in_wifi_block = false;
    for line in ports.lines() {
        let line = line.trim();
        if let Some(port) = line.strip_prefix("Hardware Port:") {
            in_wifi_block = port.trim() == "Wi-Fi";
        } else if in_wifi_block && let Some(device) = line.strip_prefix("Device:") {
            return Some(device.trim());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript- and CLI-backed functions need macOS, so they are verified
    // by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "system_set_volume",
                "system_mute",
                "system_unmute",
                "system_dark_mode",
                "system_sleep",
                "system_screensaver",
                "system_set_wallpaper",
                "system_notify",
                "system_keep_awake",
                "system_allow_sleep",
                "system_wifi"
            ]
        );
    }

    #[test]
    fn set_volume_without_level_is_error_not_panic() {
        let out = dispatch("system_set_volume", &serde_json::json!({})).unwrap();
        assert!(out.contains("error"), "expected error body, got {out}");
    }

    #[test]
    fn missing_fields_return_errors_not_panics() {
        for (tool, body) in [
            ("system_dark_mode", serde_json::json!({})),
            ("system_set_wallpaper", serde_json::json!({})),
            ("system_notify", serde_json::json!({ "title": "t" })),
            ("system_keep_awake", serde_json::json!({})),
            ("system_wifi", serde_json::json!({})),
        ] {
            let out = dispatch(tool, &body).expect("dispatch owns the tool");
            assert!(
                out.contains("missing"),
                "{tool}: expected missing, got {out}"
            );
        }
    }

    #[test]
    fn dark_mode_rejects_unknown_mode() {
        let out = dispatch("system_dark_mode", &serde_json::json!({ "mode": "blue" })).unwrap();
        assert!(out.contains("unknown mode"));
    }

    #[test]
    fn dispatch_unknown_returns_none() {
        assert!(dispatch("not_a_system_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn wifi_device_finds_the_wifi_port() {
        let ports = "Hardware Port: Ethernet\nDevice: en1\nEthernet Address: aa\n\n\
                     Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: bb\n";
        assert_eq!(wifi_device(ports), Some("en0"));
    }

    #[test]
    fn wifi_device_handles_no_wifi_port() {
        let ports = "Hardware Port: Ethernet\nDevice: en1\n";
        assert_eq!(wifi_device(ports), None);
        assert_eq!(wifi_device(""), None);
    }
}
