//! winit-based cursor overlay. Covers Windows, macOS, and X11.
//!
//! Drawing pipeline mirrors the Cairo path in hyprland.rs:
//!   PNG sprite → tiny-skia Pixmap → drawn with Transform::from_translate
//!   at sub-pixel f32 coords + bilinear sampling → blitted to softbuffer's
//!   surface as 0RGB u32 pixels.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId, WindowLevel};

// ── Portable constants (verbatim from hyprland.rs) ──────────────────────────
// Time for cursor lag to halve. 91.7ms reproduces the previous 500Hz × 0.015
// feel under a delta-time formulation, so the cursor is equally snappy at
// 60Hz, 144Hz, or 500Hz tick rates.
const SMOOTHING_HALF_LIFE: f64 = 0.0917;
const Y_OFFSET: i32 = -50;
const X_OFFSET: i32 = 10;
const POINT_DURATION: Duration = Duration::from_secs(3);
const CURSOR_DISPLAY_SIZE: f32 = 18.0;

const CURSOR_PNG: &[u8] = include_bytes!("../../assets/cursor.png");

// ── Portable thread-safe channel ────────────────────────────────────────────
static CURSOR_SENDER: OnceLock<Sender<(i32, i32)>> = OnceLock::new();

/// Ask the cursor to fly to (x, y) and sit there for ~3 seconds, then resume
/// following the mouse. Callable from any thread. No-op if `cursor()` hasn't
/// been initialized yet.
pub fn point_at(x: i32, y: i32) {
    if let Some(sender) = CURSOR_SENDER.get() {
        let _ = sender.send((x, y));
    }
}

struct CursorApp {
    attrs: WindowAttributes,
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    /// Native-size sprite, decoded once at startup.
    sprite: Pixmap,
    /// Scale factor from native sprite size → CURSOR_DISPLAY_SIZE.
    sprite_scale: f32,
    /// Fullscreen canvas we draw into each frame, then copy to softbuffer.
    canvas: Option<Pixmap>,
    receiver: Receiver<(i32, i32)>,
    cursor_x: f64,
    cursor_y: f64,
    override_target: Option<(i32, i32, Instant)>,
    last_tick: Option<Instant>,
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
        }
        let Some(canvas) = self.canvas.as_mut() else {
            return;
        };

        // Run one tick to advance position.
        let next = tick(
            &self.receiver,
            &mut self.cursor_x,
            &mut self.cursor_y,
            &mut self.override_target,
            &mut self.last_tick,
        );

        // Clear to transparent. Drawing happens only if tick returned a position.
        canvas.fill(tiny_skia::Color::TRANSPARENT);
        if let Some((x, y)) = next {
            let transform = Transform::from_scale(self.sprite_scale, self.sprite_scale)
                .post_translate(x as f32, y as f32);
            canvas.draw_pixmap(
                0,
                0,
                self.sprite.as_ref(),
                &PixmapPaint {
                    quality: FilterQuality::Bilinear,
                    ..Default::default()
                },
                transform,
                None,
            );
        }

        // Copy tiny-skia (RGBA premultiplied) → softbuffer (packed u32).
        // Pack alpha into the high byte: softbuffer's docs say it's 0RGB, but
        // Windows 11's DWM is reported to honor the high byte as alpha, giving
        // us per-pixel transparency. No-op on platforms that genuinely treat
        // the byte as zero-padding.
        let mut buffer = surface.buffer_mut().expect("buffer_mut");
        let src = canvas.data();
        for (dst, chunk) in buffer.iter_mut().zip(src.chunks_exact(4)) {
            *dst = u32::from_be_bytes([chunk[3], chunk[0], chunk[1], chunk[2]]);
        }
        buffer.present().expect("buffer present");

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

    let sprite = Pixmap::decode_png(CURSOR_PNG).expect("decode cursor.png");
    let sprite_scale = CURSOR_DISPLAY_SIZE / sprite.width() as f32;

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
        sprite,
        sprite_scale,
        canvas: None,
        receiver,
        cursor_x: initial_x as f64,
        cursor_y: initial_y as f64,
        override_target: None,
        last_tick: None,
        frame_count: 0,
        fps_log_start: None,
    };

    event_loop.run_app(&mut app).expect("run_app failed");
    std::process::exit(0);
}

/// Drains pending point_at commands, picks a target (override or mouse),
/// runs the smoothing step, and returns the next (x, y) to render.
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
            let mouse = crate::mouse::mouse_movement()
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
