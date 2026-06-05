//! Serialized executor for the input actions Claude can request (click, type,
//! key, scroll). Public functions enqueue commands; one executor thread drains
//! them in order and dispatches to the platform input backend in `crate::input`.
//! Fire-and-forget: errors get logged but don't propagate, since these run
//! inside the streaming SSE callback where blocking would delay tokens.
//!
//! Window/app management (open URL, launch, focus, inventory) lives in
//! `crate::desktop`.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::thread;

/// Commands serialized through one executor thread to preserve ordering
/// across async SSE callbacks (e.g. click-then-type must stay in that
/// order even though both arrive via the same callback fast enough to
/// race ydotool's per-call latency).
enum InputCmd {
    /// Moves the OS cursor to (x, y) and fires a left button down+up.
    Click { x: i64, y: i64 },
    /// Trailing `\n` in `text` submits (fires Enter after typing).
    Type { text: String },
    /// `combo` is human syntax like "Return", "ctrl+a".
    Key { combo: String },
    /// `amount` is wheel-clicks; mapped to arrow-key presses by the backend.
    Scroll { direction: String, amount: u32 },
}

/// Set by `init_input_executor` at startup. OnceLock makes the static
/// callable from anywhere without plumbing and makes double-init a no-op.
static INPUT_TX: OnceLock<Sender<InputCmd>> = OnceLock::new();

/// Move the system mouse to (x, y) and synthesize a left-button click.
/// Enqueues onto the input executor; returns immediately. The overlay
/// animation (GTK thread) and the click work (executor thread) run in
/// parallel so they look simultaneous to the user, but click-then-type
/// sequences within the executor stay strictly ordered.
pub fn click_at(x: i64, y: i64) {
    eprintln!("[action:click_at] queueing click at ({}, {})", x, y);
    enqueue(InputCmd::Click { x, y });
}

/// Type text into the currently-focused field. Embed a trailing \n in
/// `text` if you want Enter to fire after (search submission, message
/// send). Enqueues onto the input executor so it always lands after any
/// pending click that came before it.
pub fn type_text(text: &str) {
    eprintln!("[action:type_text] queueing type ({} chars)", text.len());
    enqueue(InputCmd::Type {
        text: text.to_string(),
    });
}

/// Press a key or key combination ("Return", "Tab", "Escape", "ctrl+a",
/// "ctrl+f", etc.). Enqueues onto the input executor so it's serialized
/// against pending clicks and types.
pub fn press_key(combo: &str) {
    eprintln!("[action:press_key] queueing key '{}'", combo);
    enqueue(InputCmd::Key {
        combo: combo.to_string(),
    });
}

/// Scroll by sending repeated arrow-key presses. Wayland has no clean
/// "scroll at point" primitive, so we approximate with keyboard scrolling.
/// Works in browsers, terminals, file managers, anywhere arrow keys move
/// the viewport. The `amount` parameter is roughly "wheel clicks".
pub fn scroll(direction: &str, amount: u32) {
    eprintln!("[action:scroll] queueing scroll {} × {}", direction, amount);
    enqueue(InputCmd::Scroll {
        direction: direction.to_string(),
        amount,
    });
}

/// Drops the command (loud log) if `init_input_executor` hasn't run.
/// Indicates a startup-order regression, not normal flow.
fn enqueue(cmd: InputCmd) {
    match INPUT_TX.get() {
        Some(tx) => {
            let _ = tx.send(cmd);
        }
        None => {
            eprintln!(
                "[action] input executor not initialized; \
                 dropping command. Call init_input_executor() at startup."
            );
        }
    }
}

/// Starts the single-threaded input executor. Must be called once at
/// startup, before any click/type/key/scroll can fire. Idempotent: a
/// second call is silently ignored. Each command dispatches to the
/// OS-specific backend in `crate::input`.
pub fn init_input_executor() {
    let (tx, rx) = channel::<InputCmd>();
    if INPUT_TX.set(tx).is_err() {
        return;
    }
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                InputCmd::Click { x, y } => crate::input::exec_click(x, y),
                InputCmd::Type { text } => crate::input::exec_type(&text),
                InputCmd::Key { combo } => crate::input::exec_key(&combo),
                InputCmd::Scroll { direction, amount } => {
                    crate::input::exec_scroll(&direction, amount)
                }
            }
        }
    });
}

/// Startup probe: check whether input injection is available on this platform.
/// Delegates to the OS-specific backend in `crate::input`. Never fails startup;
/// pointing, opening URLs, and launching apps still work without it.
pub fn check_input_injection_available() {
    crate::input::check_available();
}
