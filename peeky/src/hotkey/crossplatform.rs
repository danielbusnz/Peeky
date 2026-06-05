//! Cross-platform hotkey via the global-hotkey crate.
//!
//! `init()` creates the manager on the calling thread (must be the main
//! thread on macOS). `poll()` drains pending events into `RECORDING`;
//! call it every iteration of the main event loop (ai_cursor/winit.rs does
//! this in RedrawRequested). On Windows, winit's main thread pumps the
//! Win32 message queue automatically so events flow without our help.
//!
//! Platforms: Windows, macOS, Linux X11. Hyprland/Wayland uses hyprland.rs.

#[cfg(target_os = "macos")]
use global_hotkey::hotkey::Modifiers;
use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::{AtomicBool, Ordering};

use super::backend::HotkeyBackend;

static RECORDING: AtomicBool = AtomicBool::new(false);

/// Ctrl+Space for macOS (easy to hit one-handed).
/// Plain Insert on other platforms matches the Hyprland config.
#[cfg(target_os = "macos")]
fn build_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL), Code::Space)
}
#[cfg(not(target_os = "macos"))]
fn build_hotkey() -> HotKey {
    HotKey::new(None, Code::Insert)
}

/// Cross-platform polling backend. Zero-sized; never instantiated.
pub struct Backend;

impl HotkeyBackend for Backend {
    /// Register the global hotkey. MUST be called from the main thread on
    /// macOS; harmless on Windows/X11. The manager is intentionally leaked
    /// so it lives for the program's lifetime; we only need the receiver
    /// from then on.
    fn init() -> std::io::Result<()> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| std::io::Error::other(format!("manager: {}", e)))?;
        manager
            .register(build_hotkey())
            .map_err(|e| std::io::Error::other(format!("register: {}", e)))?;
        Box::leak(Box::new(manager));
        eprintln!("[hotkey] registered (global)");
        Ok(())
    }

    /// Drain pending hotkey events into RECORDING. Non-blocking. Called from
    /// the main event loop (winit's RedrawRequested fires hundreds of times
    /// per second under ControlFlow::Poll, giving us plenty of frequency).
    fn poll() {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            match event.state {
                HotKeyState::Pressed => {
                    eprintln!("[hotkey] press");
                    RECORDING.store(true, Ordering::Relaxed);
                }
                HotKeyState::Released => {
                    eprintln!("[hotkey] release");
                    RECORDING.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    fn is_recording() -> bool {
        RECORDING.load(Ordering::Relaxed)
    }

    /// 1ms poll keeps latency well below human-perceptual without burning CPU.
    fn wait_for_press() {
        while !Self::is_recording() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// No-op: this backend exposes state through `is_recording`/`poll`, and the
    /// winit overlay reads that directly rather than via callbacks. Present so
    /// the contract holds across every backend.
    fn on_press(_f: Box<dyn Fn() + Send + Sync + 'static>) {}

    /// No-op. See [`on_press`](Self::on_press).
    fn on_release(_f: Box<dyn Fn() + Send + Sync + 'static>) {}
}
