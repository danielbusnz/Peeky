//! winit-based cursor overlay. Covers Windows, macOS, and X11.
//!
//! Drawing pipeline mirrors the Cairo path in hyprland.rs:
//!   drawable (sprite/soundwave/spinner) → tiny-skia Pixmap → Renderer.
//! The renderer is softbuffer on Linux/Windows and wgpu on macOS; see
//! `renderer.rs` for why.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use super::CursorState;
use super::common::{
    self, CURSOR_DISPLAY_SIZE, CURSOR_PNG, CURSOR_SENDER, DirtyRect, STATE_SENDER, tick,
};
use super::platform;
use super::renderer::Renderer;
use crate::painter::{DrawSkia, LoadingSpinner, Soundwave, SpriteSkia};

struct CursorApp {
    attrs: WindowAttributes,
    window: Option<Arc<Window>>,
    surface: Option<Renderer>,
    /// Canvas we draw into each frame, then hand to the renderer.
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

        // Platform-specific window sizing (macOS: fill screen without fullscreen mode)
        platform::post_window_create(event_loop, &window);

        // Configure transparency (macOS: NSWindow non-opaque with clear background)
        platform::configure_transparency(&window);

        // Construct the platform's renderer (softbuffer on Linux/Windows, wgpu on macOS).
        let renderer = Renderer::new(window.clone()).expect("renderer init");

        self.surface = Some(renderer);
        self.window = Some(window.clone());

        // Request first redraw
        window.request_redraw();
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
        let (Some(window), Some(renderer)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Resize renderer + reallocate the tiny-skia canvas if window grew.
        renderer
            .resize(size.width, size.height)
            .expect("renderer resize");
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
            common::set_state(CursorState::Listening);
        }
        if !recording && self.was_recording {
            common::set_state(CursorState::Loading);
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

        // Run one tick to advance position.
        let next = tick(
            &self.receiver,
            &mut self.cursor_x,
            &mut self.cursor_y,
            &mut self.override_target,
            &mut self.last_tick,
        );

        // Platform-specific coordinate scaling (macOS: logical to physical for Retina)
        let next = next.map(|pos| platform::scale_cursor_position(window, pos));

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

        let present_dirty = if let Some(rect) = dirty {
            let rect = rect.clamp(canvas_w, canvas_h);
            if rect.is_empty() {
                None
            } else {
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
                Some(rect)
            }
        } else {
            None
        };

        renderer
            .present(canvas, present_dirty)
            .expect("renderer present");

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

    // Common attributes with platform-specific fullscreen handling.
    // macOS skips fullscreen (kills transparency) and sizes manually in resumed().
    // NOTE: We don't set WindowLevel here because on macOS we configure
    // NSWindow.level directly via objc messaging in configure_window_transparency
    // to use screenSaver level (1000) which works better with fullscreen apps.
    let attrs = Window::default_attributes()
        .with_transparent(true)
        .with_decorations(false);
    #[cfg(not(target_os = "macos"))]
    let attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop);
    let attrs = platform::apply_window_attrs(attrs);

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

// Re-export public API from common. `set_state` is only used on hyprland
// builds (via orchestrator), but we re-export it here too so external code
// can stay backend-agnostic.
#[allow(unused_imports)]
pub use common::{point_at, set_state};
