#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod agent;
mod integrations;
mod invite;
mod keychain;
mod keys;
mod proxy_contract;

use tauri::Manager;

fn main() {
    // Dev escape hatch: AEGIS_SHOW_SIGNIN=1 forces the sign-in window so the
    // login flow can be exercised without going through (or resetting)
    // onboarding. Skips the onboarded-spawn shortcut below.
    let show_signin = std::env::var_os("AEGIS_SHOW_SIGNIN").is_some();

    // If already onboarded, spawn aegis directly and exit (no UI).
    if !show_signin && invite::is_onboarded() {
        if let Err(e) = agent::spawn_aegis() {
            eprintln!("[console] {e}");
        }
        return;
    }

    // webkit2gtk's DMABUF renderer crashes against Hyprland and several
    // other Wayland compositors with "Error 71 (Protocol error)". Disabling
    // it forces a software path that works everywhere. Harmless on non-Linux
    // platforms but gated since the env var only exists on Linux.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let builder = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            agent::spawn_aegis,
            invite::save_invite_code,
            invite::mark_onboarded,
            invite::verify_invite_code,
            keys::save_api_keys,
            keys::api_keys_status,
            keys::verify_api_keys,
            account::github_sign_in,
            account::account_status,
            account::sign_out,
            integrations::integrations_status
        ])
        .setup(move |app| {
            // With the dev flag, surface the (normally hidden) settings window
            // and hide the onboarding window, so the console opens straight to login.
            if show_signin {
                if let Some(settings) = app.get_webview_window("settings") {
                    let _ = settings.show();
                    let _ = settings.set_focus();
                }
                if let Some(onboarding) = app.get_webview_window("onboarding") {
                    let _ = onboarding.hide();
                }
            }
            Ok(())
        });

    // macOS only: the permission plugin lets onboarding prompt for mic, screen
    // recording, and accessibility before the agent spawns. Compiled out
    // elsewhere, so the Linux/Windows builds are unchanged.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_macos_permissions::init());

    builder
        .run(tauri::generate_context!())
        .expect("error running console");
}
