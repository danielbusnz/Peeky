//! Cross-platform hotkey via the global-hotkey crate.
//!
//! `init()` creates the manager on the calling thread (must be the main
//! thread on macOS). `poll()` drains pending events into `RECORDING`;
//! call it every iteration of the main event loop (cursor/winit.rs does
//! this in RedrawRequested). On Windows, winit's main thread pumps the
//! Win32 message queue automatically so events flow without our help.
//!
//! Platforms: Windows, macOS, Linux X11. Hyprland/Wayland uses unix_signals.rs.

use global_hotkey::hotkey::{Code, HotKey};
#[cfg(target_os = "macos")]
use global_hotkey::hotkey::Modifiers;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::{AtomicBool, Ordering};

static RECORDING: AtomicBool = AtomicBool::new(false);

/// macOS keyboards usually lack a physical Insert key, so we use a chord
/// the OS doesn't already bind. Everywhere else, plain Insert matches the
/// Hyprland config.
#[cfg(target_os = "macos")]
fn build_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::Space)
}
#[cfg(not(target_os = "macos"))]
fn build_hotkey() -> HotKey {
    HotKey::new(None, Code::Insert)
}

/// Register the global hotkey. MUST be called from the main thread on
/// macOS; harmless on Windows/X11. The manager is intentionally leaked
/// so it lives for the program's lifetime; we only need the receiver
/// from then on.
pub fn init() -> std::io::Result<()> {
    let manager =
        GlobalHotKeyManager::new().map_err(|e| std::io::Error::other(format!("manager: {}", e)))?;
    manager
        .register(build_hotkey())
        .map_err(|e| std::io::Error::other(format!("register: {}", e)))?;
    Box::leak(Box::new(manager));
    eprintln!("[hotkey] registered (global)");
    Ok(())
}

/// Drain pending hotkey events into RECORDING. Non-blocking. Call from
/// the main event loop (winit's RedrawRequested fires hundreds of times
/// per second under ControlFlow::Poll, giving us plenty of frequency).
pub fn poll() {
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

/// True while the hotkey is held. Cheap; reads a relaxed atomic.
pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// Blocks the calling thread until the hotkey is pressed. 1ms poll
/// keeps latency well below human-perceptual without burning CPU.
pub fn wait_for_press() {
    while !is_recording() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
