//! Cross-platform hotkey via the global-hotkey crate. Mirrors the public
//! API of unix_signals.rs so callers don't have to change.
//!
//! Platforms: Windows, macOS, Linux X11. Wayland uses unix_signals.rs.

use global_hotkey::hotkey::{Code, HotKey};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static RECORDING: AtomicBool = AtomicBool::new(false);

/// Spawn the hotkey listener thread. The thread owns the
/// `GlobalHotKeyManager` (required by the crate's threading rules), pumps
/// Win32 messages on Windows so the OS can deliver WM_HOTKEY, and updates
/// `RECORDING` on press/release.
pub fn init() -> std::io::Result<()> {
    thread::spawn(|| {
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[hotkey] manager init failed: {}", e);
                return;
            }
        };
        let hotkey = HotKey::new(None, Code::Insert);
        if let Err(e) = manager.register(hotkey) {
            eprintln!("[hotkey] register failed: {}", e);
            return;
        }
        eprintln!("[hotkey] Insert registered (global)");

        let receiver = GlobalHotKeyEvent::receiver();
        loop {
            while let Ok(event) = receiver.try_recv() {
                match event.state {
                    HotKeyState::Pressed => {
                        eprintln!("[hotkey] Insert press");
                        RECORDING.store(true, Ordering::Relaxed);
                    }
                    HotKeyState::Released => {
                        eprintln!("[hotkey] Insert release");
                        RECORDING.store(false, Ordering::Relaxed);
                    }
                }
            }
            pump();
        }
    });
    Ok(())
}

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

pub fn wait_for_press() {
    while !is_recording() {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// On Windows, global-hotkey delivers WM_HOTKEY through the thread's
/// message queue. We must pump it ourselves on this dedicated thread.
/// On Mac/Linux X11 the underlying impl handles delivery internally, so
/// we just sleep briefly between receiver polls.
#[cfg(target_os = "windows")]
fn pump() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    unsafe {
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    thread::sleep(Duration::from_millis(2));
}

#[cfg(not(target_os = "windows"))]
fn pump() {
    thread::sleep(Duration::from_millis(2));
}
