//! macOS input injection via CoreGraphics (CGEvent). No external tools.

use std::thread;
use std::time::Duration;

use super::backend::InputInjector;

/// CoreGraphics backend. Zero-sized; never instantiated.
pub struct Backend;

impl InputInjector for Backend {
    fn exec_click(x: i64, y: i64) {
        use objc2_core_foundation::CGPoint;
        use objc2_core_graphics::{
            CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, CGWarpMouseCursorPosition,
        };

        let point = CGPoint {
            x: x as f64,
            y: y as f64,
        };

        // Move cursor to position
        CGWarpMouseCursorPosition(point);

        // Small delay for cursor position to settle
        thread::sleep(Duration::from_millis(30));

        // Create and post mouse down event
        if let Some(down_event) =
            CGEvent::new_mouse_event(None, CGEventType::LeftMouseDown, point, CGMouseButton::Left)
        {
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down_event));
        }

        // Create and post mouse up event
        if let Some(up_event) =
            CGEvent::new_mouse_event(None, CGEventType::LeftMouseUp, point, CGMouseButton::Left)
        {
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up_event));
        }
    }

    fn exec_type(text: &str) {
        use objc2_core_graphics::{CGEvent, CGEventTapLocation};

        // Wait for focus to settle after a preceding click
        thread::sleep(Duration::from_millis(80));

        // Check if text ends with newline (submit)
        let (text_to_type, needs_enter) = if text.ends_with('\n') {
            (&text[..text.len() - 1], true)
        } else {
            (text, false)
        };

        // Confirm the action reached OS-level execution (not just the queue in
        // actions.rs). The literal text is only logged under AEGIS_INPUT_DEBUG,
        // since it can contain anything the user types, passwords included.
        if std::env::var("AEGIS_INPUT_DEBUG").is_ok() {
            eprintln!(
                "[input:type] injecting {} char(s): {:?} (enter={})",
                text_to_type.chars().count(),
                text_to_type,
                needs_enter
            );
        } else {
            eprintln!(
                "[input:type] injecting {} char(s) (enter={})",
                text_to_type.chars().count(),
                needs_enter
            );
        }

        // Type one character at a time, posting a key-down AND a key-up for
        // each with the unicode string set on both. A single event carrying
        // the whole string is unreliable (some apps take only the first char),
        // and a key-down with no matching key-up often isn't committed by the
        // receiving app, which is why bulk typing silently dropped text.
        for ch in text_to_type.chars() {
            let mut buf = [0u16; 2];
            let utf16 = ch.encode_utf16(&mut buf);
            for key_down in [true, false] {
                if let Some(event) = CGEvent::new_keyboard_event(None, 0, key_down) {
                    unsafe {
                        CGEvent::keyboard_set_unicode_string(
                            Some(&event),
                            utf16.len() as u64,
                            utf16.as_ptr(),
                        );
                    }
                    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
                }
            }
        }

        // Press Enter if text ended with newline
        if needs_enter {
            thread::sleep(Duration::from_millis(30));
            // Key code 36 = Return on macOS
            if let Some(down) = CGEvent::new_keyboard_event(None, 36, true) {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
            }
            if let Some(up) = CGEvent::new_keyboard_event(None, 36, false) {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
            }
        }

        eprintln!("[input:type] injection complete");
    }

    fn exec_key(combo: &str) {
        use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};

        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();

        // Collect modifiers and the main key
        let mut flags = CGEventFlags::empty();
        let mut main_key: Option<&str> = None;

        for part in parts {
            let lower = part.to_lowercase();
            match lower.as_str() {
                "ctrl" | "control" | "leftctrl" => flags |= CGEventFlags::MaskControl,
                "shift" | "leftshift" | "rightshift" => flags |= CGEventFlags::MaskShift,
                "alt" | "option" | "leftalt" | "rightalt" => flags |= CGEventFlags::MaskAlternate,
                "super" | "meta" | "win" | "cmd" | "command" => flags |= CGEventFlags::MaskCommand,
                _ => main_key = Some(part),
            }
        }

        let Some(key) = main_key else {
            eprintln!("[action:key] no main key in combo '{}'", combo);
            return;
        };

        // Map key name to macOS key code
        let key_code: u16 = match key.to_lowercase().as_str() {
            "esc" | "escape" => 53,
            "tab" => 48,
            "enter" | "return" => 36,
            "backspace" => 51,
            "delete" | "del" => 117,
            "space" => 49,
            "up" | "arrowup" => 126,
            "down" | "arrowdown" => 125,
            "left" | "arrowleft" => 123,
            "right" | "arrowright" => 124,
            "home" => 115,
            "end" => 119,
            "pageup" | "page_up" => 116,
            "pagedown" | "page_down" => 121,
            "f1" => 122,
            "f2" => 120,
            "f3" => 99,
            "f4" => 118,
            "f5" => 96,
            "f6" => 97,
            "f7" => 98,
            "f8" => 100,
            "f9" => 101,
            "f10" => 109,
            "f11" => 103,
            "f12" => 111,
            // Letters a-z (macOS key codes)
            "a" => 0,
            "b" => 11,
            "c" => 8,
            "d" => 2,
            "e" => 14,
            "f" => 3,
            "g" => 5,
            "h" => 4,
            "i" => 34,
            "j" => 38,
            "k" => 40,
            "l" => 37,
            "m" => 46,
            "n" => 45,
            "o" => 31,
            "p" => 35,
            "q" => 12,
            "r" => 15,
            "s" => 1,
            "t" => 17,
            "u" => 32,
            "v" => 9,
            "w" => 13,
            "x" => 7,
            "y" => 16,
            "z" => 6,
            // Numbers 0-9
            "0" => 29,
            "1" => 18,
            "2" => 19,
            "3" => 20,
            "4" => 21,
            "5" => 23,
            "6" => 22,
            "7" => 26,
            "8" => 28,
            "9" => 25,
            _ => {
                eprintln!(
                    "[action:key] unrecognized key '{}' in combo '{}'",
                    key, combo
                );
                return;
            }
        };

        thread::sleep(Duration::from_millis(50));

        // Key down with modifiers
        if let Some(down) = CGEvent::new_keyboard_event(None, key_code, true) {
            CGEvent::set_flags(Some(&down), flags);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
        }

        // Key up with modifiers
        if let Some(up) = CGEvent::new_keyboard_event(None, key_code, false) {
            CGEvent::set_flags(Some(&up), flags);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
        }
    }

    fn exec_scroll(direction: &str, amount: u32) {
        use objc2_core_graphics::{CGEvent, CGEventTapLocation};

        // Map direction to macOS key code (arrow keys)
        let key_code: u16 = match direction.to_lowercase().as_str() {
            "down" => 125,  // down arrow
            "up" => 126,    // up arrow
            "left" => 123,  // left arrow
            "right" => 124, // right arrow
            other => {
                eprintln!(
                    "[action:scroll] unknown direction '{}', defaulting to down",
                    other
                );
                125
            }
        };
        // 3 presses per wheel-click, capped at 30 to prevent hanging
        let presses = (amount.saturating_mul(3)).clamp(1, 30);

        for _ in 0..presses {
            // Key down
            if let Some(down) = CGEvent::new_keyboard_event(None, key_code, true) {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
            }
            // Key up
            if let Some(up) = CGEvent::new_keyboard_event(None, key_code, false) {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn check_available() {
        use objc2_core_graphics::CGEvent;

        // Try to create a test event - this will fail if Accessibility is denied
        match CGEvent::new_keyboard_event(None, 0, true) {
            Some(_) => {
                eprintln!(
                    "[startup] CoreGraphics input available. click actions will fire real input"
                );
            }
            None => {
                eprintln!(
                    "[startup] WARNING: CoreGraphics event creation failed. Click actions will move the\n\
                     \toverlay but NOT inject a real click. To enable:\n\
                     \t  System Preferences → Privacy & Security → Accessibility\n\
                     \t  Add and enable your terminal app (Terminal, iTerm2, etc.)"
                );
            }
        }
    }
}
