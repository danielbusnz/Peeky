//! winit-based cursor overlay. Covers Windows, macOS, and X11.
//!
//! Drawing pipeline mirrors the Cairo path in hyprland.rs:
//!   drawable (sprite/soundwave/spinner) → tiny-skia Pixmap → dirty-region
//!   copy to softbuffer's surface as 0RGB u32 pixels.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

use super::CursorState;
use crate::painter::{DrawSkia, LoadingSpinner, Soundwave, SpriteSkia};

// ── Portable constants (mirrors hyprland.rs) ────────────────────────────────
// Time for cursor lag to halve. 91.7ms reproduces the previous 500Hz × 0.015
// feel under a delta-time formulation, so the cursor is equally snappy at
// 60Hz, 144Hz, or 500Hz tick rates.
const SMOOTHING_HALF_LIFE: f64 = 0.0917;
const Y_OFFSET: i32 = -70;
const X_OFFSET: i32 = 20;
const POINT_DURATION: Duration = Duration::from_secs(3);
const CURSOR_DISPLAY_SIZE: f64 = 18.0;

const CURSOR_PNG: &[u8] = include_bytes!("../../assets/cursor.png");

// ── Portable thread-safe channels ───────────────────────────────────────────
static CURSOR_SENDER: OnceLock<Sender<(i32, i32)>> = OnceLock::new();
static STATE_SENDER: OnceLock<Sender<CursorState>> = OnceLock::new();

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
struct DirtyRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl DirtyRect {
    fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
    fn clamp(self, w: u32, h: u32) -> Self {
        Self {
            x0: self.x0.clamp(0, w as i32),
            y0: self.y0.clamp(0, h as i32),
            x1: self.x1.clamp(0, w as i32),
            y1: self.y1.clamp(0, h as i32),
        }
    }
    fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

struct CursorApp {
    attrs: WindowAttributes,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    /// Fullscreen canvas we draw into each frame, then copy to softbuffer.
    canvas: Option<Pixmap>,
    /// What we're drawing right now. Swapped on CursorState transitions.
    drawable: Box<dyn DrawSkia>,
    receiver: Receiver<(i32, i32)>,
    state_receiver: Receiver<CursorState>,
    cursor_x: f64,
    cursor_y: f64,
    override_target: Option<(i32, i32, Instant)>,
    last_tick: Option<Instant>,
    /// Bounding box of where the drawable was painted on the previous frame,
    /// so we know which pixels to clear before drawing the new one. `None`
    /// means nothing was drawn last frame (or the canvas was just allocated).
    last_sprite_rect: Option<DirtyRect>,
    /// Tracks the previous hotkey state so we can detect press/release edges
    /// and send the matching CursorState transitions.
    was_recording: bool,
    /// Frames rendered since `fps_log_start`. Reset every time we log.
    frame_count: u32,
    /// Start of the current 1-second FPS-counting window.
    fps_log_start: Option<Instant>,
}

impl ApplicationHandler for CursorApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(self.attrs.clone())
                .expect("create_window failed"),
        );
        window
            .set_cursor_hittest(false)
            .expect("set_cursor_hittest failed");

        let context = Context::new(window.clone()).expect("softbuffer Context");
        let surface = Surface::new(&context, window.clone()).expect("softbuffer Surface");

        self.surface = Some(surface);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // Drain any pending hotkey events. The manager lives on this
                // (main) thread per macOS's requirement; this is where its
                // events get processed.
                crate::hotkey::poll();
                self.render();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl CursorApp {
    fn render(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };

        // Resize softbuffer + reallocate the tiny-skia canvas if window grew.
        surface.resize(w, h).expect("surface resize");
        let needs_alloc = self
            .canvas
            .as_ref()
            .map(|c| c.width() != size.width || c.height() != size.height)
            .unwrap_or(true);
        if needs_alloc {
            self.canvas = Pixmap::new(size.width, size.height);
            // softbuffer's buffer may be uninitialized; force a full first pass.
            self.last_sprite_rect = None;
        }
        let Some(canvas) = self.canvas.as_mut() else {
            return;
        };
        let canvas_w = canvas.width();
        let canvas_h = canvas.height();

        // Hotkey edge detection: mirrors hyprland's on_press/on_release wiring.
        // Pushed onto the same state channel that voice.rs / external callers
        // use, so the swap logic below handles all sources uniformly.
        let recording = crate::hotkey::is_recording();
        if recording && !self.was_recording {
            set_state(CursorState::Listening);
        }
        if !recording && self.was_recording {
            set_state(CursorState::Loading);
        }
        self.was_recording = recording;

        // Drain state changes; the latest one wins.
        while let Ok(state) = self.state_receiver.try_recv() {
            self.drawable = match state {
                CursorState::Idle => {
                    Box::new(SpriteSkia::from_png(CURSOR_PNG, CURSOR_DISPLAY_SIZE))
                }
                CursorState::Listening => Box::new(Soundwave::new()),
                CursorState::Loading => Box::new(LoadingSpinner::new()),
            };
        }

        // Run one tick to advance position. The mouse_position crate
        // returns logical points on macOS but physical pixels on X11, so
        // we scale only on macOS. The canvas is sized in physical pixels
        // (window.inner_size() returns physical), so without this the
        // sprite renders in the upper-left quadrant on Retina displays.
        let next = tick(
            &self.receiver,
            &mut self.cursor_x,
            &mut self.cursor_y,
            &mut self.override_target,
            &mut self.last_tick,
        );
        #[cfg(target_os = "macos")]
        let next = next.map(|(x, y)| {
            let sf = window.scale_factor();
            (x * sf, y * sf)
        });

        // Compute the drawable's bounding box for this frame. (x, y) is the
        // visual center (matching hyprland's Drawable convention), so the box
        // is [x - w/2, x + w/2] × [y - h/2, y + h/2]. 2px of padding absorbs
        // antialias bleed on the edges.
        let (dw, dh) = self.drawable.size();
        let half_w = (dw / 2.0).ceil() as i32 + 2;
        let half_h = (dh / 2.0).ceil() as i32 + 2;
        let new_rect = next.map(|(x, y)| DirtyRect {
            x0: x.floor() as i32 - half_w,
            y0: y.floor() as i32 - half_h,
            x1: x.ceil() as i32 + half_w,
            y1: y.ceil() as i32 + half_h,
        });

        // Dirty region = union of last-frame sprite + this-frame sprite, or
        // the whole canvas if we just allocated (need to initialize softbuffer).
        let dirty = if needs_alloc {
            Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: canvas_w as i32,
                y1: canvas_h as i32,
            })
        } else {
            match (self.last_sprite_rect, new_rect) {
                (None, None) => None,
                (Some(r), None) | (None, Some(r)) => Some(r),
                (Some(a), Some(b)) => Some(a.union(b)),
            }
        };

        if let Some(rect) = dirty {
            let rect = rect.clamp(canvas_w, canvas_h);
            if !rect.is_empty() {
                // Clear the dirty region on the canvas. Zero bytes equals
                // premultiplied transparent under tiny-skia's RGBA layout.
                let stride_bytes = canvas_w as usize * 4;
                let row_start = rect.x0 as usize * 4;
                let row_end = rect.x1 as usize * 4;
                let data = canvas.data_mut();
                for y in rect.y0..rect.y1 {
                    let row = (y as usize) * stride_bytes;
                    data[row + row_start..row + row_end].fill(0);
                }

                // Draw the active drawable. tiny-skia handles clipping if it
                // extends past the canvas edge.
                if let Some((x, y)) = next {
                    self.drawable.draw_skia(canvas, x, y);
                }

                // Convert ONLY the dirty region from canvas RGBA bytes into
                // softbuffer's 0xAARRGGBB u32 pixels. Windows 11 DWM honors
                // the high byte as alpha, giving per-pixel transparency.
                let mut buffer = surface.buffer_mut().expect("buffer_mut");
                let src = canvas.data();
                let pix_stride = canvas_w as usize;
                for y in rect.y0..rect.y1 {
                    let src_row = (y as usize) * pix_stride * 4;
                    let dst_row = (y as usize) * pix_stride;
                    for x in rect.x0..rect.x1 {
                        let i = x as usize;
                        let c = &src[src_row + i * 4..src_row + i * 4 + 4];
                        buffer[dst_row + i] = u32::from_be_bytes([c[3], c[0], c[1], c[2]]);
                    }
                }
                buffer.present().expect("buffer present");
            }
        } else {
            // No sprite this frame and none last frame. Present an unchanged
            // buffer so softbuffer's frame model stays happy.
            let buffer = surface.buffer_mut().expect("buffer_mut");
            buffer.present().expect("buffer present");
        }

        self.last_sprite_rect = new_rect;

        // Rolling FPS log: count frames over a 1-second window, then print
        // and reset. Diagnostic for tuning cursor smoothness on different
        // displays. Remove once we're happy with the perceived feel.
        self.frame_count += 1;
        let now = Instant::now();
        match self.fps_log_start {
            None => self.fps_log_start = Some(now),
            Some(start) => {
                let elapsed = now.duration_since(start).as_secs_f64();
                if elapsed >= 1.0 {
                    eprintln!("[render] {:.1} fps", self.frame_count as f64 / elapsed);
                    self.frame_count = 0;
                    self.fps_log_start = Some(now);
                }
            }
        }
    }
}

/// Boot the overlay window and start the render loop. Owns the main thread
/// and never returns under normal operation.
pub fn cursor(initial_x: i32, initial_y: i32) -> ! {
    let (sender, receiver) = channel::<(i32, i32)>();
    let _ = CURSOR_SENDER.set(sender);

    let (state_sender, state_receiver) = channel::<CursorState>();
    let _ = STATE_SENDER.set(state_sender);

    let initial_drawable: Box<dyn DrawSkia> =
        Box::new(SpriteSkia::from_png(CURSOR_PNG, CURSOR_DISPLAY_SIZE));

    let attrs = Window::default_attributes()
        .with_transparent(true)
        .with_decorations(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_fullscreen(Some(Fullscreen::Borderless(None)));

    let event_loop = EventLoop::new().expect("EventLoop::new failed");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = CursorApp {
        attrs,
        window: None,
        surface: None,
        canvas: None,
        drawable: initial_drawable,
        receiver,
        state_receiver,
        cursor_x: initial_x as f64,
        cursor_y: initial_y as f64,
        override_target: None,
        last_tick: None,
        last_sprite_rect: None,
        was_recording: false,
        frame_count: 0,
        fps_log_start: None,
    };

    event_loop.run_app(&mut app).expect("run_app failed");
    std::process::exit(0);
}

/// Drains pending point_at commands, picks a target (override or mouse),
/// runs the smoothing step, and returns the next (x, y) to render as the
/// drawable's visual center.
fn tick(
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
