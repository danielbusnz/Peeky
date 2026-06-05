//! Shared constants, types, and utilities for cursor overlay implementations.

use std::sync::OnceLock;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::CursorState;

// ── Portable constants ─────────────────────────────────────────────────────
// Time for cursor lag to halve. 91.7ms reproduces the previous 500Hz × 0.015
// feel under a delta-time formulation, so the cursor is equally snappy at
// 60Hz, 144Hz, or 500Hz tick rates.
pub const SMOOTHING_HALF_LIFE: f64 = 0.0917;
// macOS: smaller Y offset since the cursor feels closer to the pointer
#[cfg(target_os = "macos")]
pub const Y_OFFSET: i32 = -20;
#[cfg(target_os = "macos")]
pub const X_OFFSET: i32 = 20;

// Linux/Windows: larger Y offset
#[cfg(not(target_os = "macos"))]
pub const Y_OFFSET: i32 = -70;
#[cfg(not(target_os = "macos"))]
pub const X_OFFSET: i32 = 20;
pub const POINT_DURATION: Duration = Duration::from_secs(3);
pub const CURSOR_DISPLAY_SIZE: f64 = 18.0;

pub const CURSOR_PNG: &[u8] = include_bytes!("../../assets/cursor.png");

// ── Thread-safe channels ───────────────────────────────────────────────────
pub static CURSOR_SENDER: OnceLock<Sender<(i32, i32)>> = OnceLock::new();
pub static STATE_SENDER: OnceLock<Sender<CursorState>> = OnceLock::new();

/// Push a state change to the cursor overlay. Callable from any thread.
/// No-op if `cursor()` hasn't been initialized yet.
pub fn set_state(state: CursorState) {
    if let Some(sender) = STATE_SENDER.get() {
        let _ = sender.send(state);
    }
}

/// Ask the cursor to fly to (x, y) and sit there for ~3 seconds, then resume
/// following the mouse. Callable from any thread. No-op if `cursor()` hasn't
/// been initialized yet.
pub fn point_at(x: i32, y: i32) {
    if let Some(sender) = CURSOR_SENDER.get() {
        let _ = sender.send((x, y));
    }
}

/// Half-open pixel rectangle: [x0, x1) × [y0, y1). Signed because the cursor
/// can sit slightly off-screen during smoothing, and we clamp into the canvas
/// before any indexing happens.
#[derive(Copy, Clone, Debug)]
pub struct DirtyRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl DirtyRect {
    pub fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
    pub fn clamp(self, w: u32, h: u32) -> Self {
        Self {
            x0: self.x0.clamp(0, w as i32),
            y0: self.y0.clamp(0, h as i32),
            x1: self.x1.clamp(0, w as i32),
            y1: self.y1.clamp(0, h as i32),
        }
    }
    pub fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// Drains pending point_at commands, picks a target (override or mouse),
/// runs the smoothing step, and returns the next (x, y) to render as the
/// drawable's visual center.
pub fn tick(
    receiver: &Receiver<(i32, i32)>,
    cursor_x: &mut f64,
    cursor_y: &mut f64,
    override_target: &mut Option<(i32, i32, Instant)>,
    last_tick: &mut Option<Instant>,
) -> Option<(f64, f64)> {
    let now = Instant::now();
    let delta_t = match *last_tick {
        Some(prev) => now.duration_since(prev).as_secs_f64(),
        None => 0.0,
    };
    *last_tick = Some(now);

    while let Ok((target_x, target_y)) = receiver.try_recv() {
        *override_target = Some((target_x, target_y, Instant::now() + POINT_DURATION));
    }

    let (target, apply_offsets) = match *override_target {
        Some((target_x, target_y, until)) if Instant::now() < until => {
            (Some((target_x as f64, target_y as f64)), false)
        }
        _ => {
            *override_target = None;
            let mouse = crate::mouse_position::mouse_movement()
                .ok()
                .map(|(mx, my)| (mx as f64, my as f64));
            (mouse, true)
        }
    };

    if let Some((target_x, target_y)) = target {
        let alpha = 1.0 - 2f64.powf(-delta_t / SMOOTHING_HALF_LIFE);
        *cursor_x += (target_x - *cursor_x) * alpha;
        *cursor_y += (target_y - *cursor_y) * alpha;
        let (ox, oy) = if apply_offsets {
            (X_OFFSET as f64, Y_OFFSET as f64)
        } else {
            (0.0, 0.0)
        };
        Some((*cursor_x + ox, *cursor_y + oy))
    } else {
        None
    }
}
