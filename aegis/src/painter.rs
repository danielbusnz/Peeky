use std::sync::OnceLock;
use std::time::Instant;

#[cfg(all(target_os = "linux", feature = "hyprland"))]
use gtk::cairo;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
use gtk::prelude::*;
#[cfg(all(target_os = "linux", feature = "hyprland"))]
use std::cell::{Cell, RefCell};
#[cfg(all(target_os = "linux", feature = "hyprland"))]
use std::rc::Rc;

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

/// Caller-provided function returning current mic RMS (0.0..=1.0). Registered
/// by `main` so painter doesn't have to depend on the audio module directly,
/// which keeps test bins (e.g. test_point) compiling without pulling in cpal.
static AUDIO_LEVEL_SOURCE: OnceLock<Box<dyn Fn() -> f32 + Send + Sync>> = OnceLock::new();

/// Register a function that returns the current mic level. Falls back to 0.0
/// (silence) if no source has been registered, which is what test bins get.
pub fn set_audio_level_source(f: impl Fn() -> f32 + Send + Sync + 'static) {
    let _ = AUDIO_LEVEL_SOURCE.set(Box::new(f));
}

fn current_audio_level() -> f64 {
    AUDIO_LEVEL_SOURCE.get().map(|f| f() as f64).unwrap_or(0.0)
}

// ── Soundwave constants (cursor-scale) ───────────────────────────────────────
const N_BARS: usize = 5;
const BAR_WIDTH: f64 = 3.0;
const BAR_GAP: f64 = 1.5;
const MIN_HEIGHT: f64 = 6.0;
const MAX_HEIGHT: f64 = 28.0;
const SCROLL_SPEED: f64 = 0.7;
const CORNER_RADIUS: f64 = 1.5;
const COLOR: (f64, f64, f64, f64) = (1.00, 0.55, 0.00, 0.95);
const HARMONICS: [(f64, f64, f64); 3] = [(1.5, 0.0, 0.55), (3.1, 1.0, 0.30), (5.7, 2.4, 0.15)];
/// Bell-curve silhouette floor. 0.0 = end bars shrink to nothing; 1.0 = all
/// bars the same height (no curve). 0.4 ≈ tidy half-circle pyramid.
const SHAPE_FLOOR: f64 = 0.4;

/// Animated soundwave shown while Aegis is in the listening state.
pub struct Soundwave {
    start: Instant,
}

impl Soundwave {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// One per visible bar, in left-to-right order, in local coords with the
    /// soundwave's visual center at (0, 0). Both backends consume this.
    fn bars(&self) -> [(f64, f64, f64, f64); N_BARS] {
        let t = self.start.elapsed().as_secs_f64();
        let (w, _h) = soundwave_size();
        let origin_x = -w / 2.0;
        let weight_sum: f64 = HARMONICS.iter().map(|h| h.2).sum();
        let envelope = (0.3 + current_audio_level() * 2.0).min(1.0);

        let mut out = [(0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64); N_BARS];
        for (i, slot) in out.iter_mut().enumerate() {
            let u = i as f64 / (N_BARS - 1) as f64;
            let scrolled = u + t * SCROLL_SPEED;
            let mut raw = 0.0_f64;
            for (freq, phase, weight) in HARMONICS {
                let theta = scrolled * freq * std::f64::consts::TAU + phase;
                raw += theta.sin() * (weight / weight_sum);
            }
            let unit = (raw + 1.0) / 2.0;
            let shape = SHAPE_FLOOR + (1.0 - SHAPE_FLOOR) * (u * std::f64::consts::PI).sin();
            let bar_h = (MIN_HEIGHT + (MAX_HEIGHT - MIN_HEIGHT) * unit * envelope) * shape;
            let bx = origin_x + i as f64 * (BAR_WIDTH + BAR_GAP);
            let by = -bar_h / 2.0;
            *slot = (bx, by, BAR_WIDTH, bar_h);
        }
        out
    }
}

impl Default for Soundwave {
    fn default() -> Self {
        Self::new()
    }
}

fn soundwave_size() -> (f64, f64) {
    let w = N_BARS as f64 * BAR_WIDTH + (N_BARS - 1) as f64 * BAR_GAP;
    (w, MAX_HEIGHT)
}

// ── LoadingSpinner constants (cursor-scale) ──────────────────────────────────
const SPINNER_N_BARS: usize = 12;
const SPINNER_INNER_RADIUS: f64 = 8.0;
const SPINNER_BAR_LENGTH: f64 = 7.0;
const SPINNER_BAR_WIDTH: f64 = 2.5;
const SPINNER_ROTATION_HZ: f64 = 1.0;
const SPINNER_ALPHA_FLOOR: f64 = 0.12;
const SPINNER_CORNER_RADIUS: f64 = 1.25;
const SPINNER_COLOR: (f64, f64, f64) = (1.00, 0.55, 0.00);

/// iOS-style radial spinner shown while Aegis is processing (Loading state).
pub struct LoadingSpinner {
    start: Instant,
}

impl LoadingSpinner {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// One per bar, in (angle_radians, alpha) form. Both backends consume this.
    fn bars(&self) -> [(f64, f64); SPINNER_N_BARS] {
        let t = self.start.elapsed().as_secs_f64();
        let head = (t * SPINNER_ROTATION_HZ * SPINNER_N_BARS as f64) % SPINNER_N_BARS as f64;
        let mut out = [(0.0_f64, 0.0_f64); SPINNER_N_BARS];
        for (i, slot) in out.iter_mut().enumerate() {
            let dist = (head - i as f64).rem_euclid(SPINNER_N_BARS as f64);
            let alpha = SPINNER_ALPHA_FLOOR
                + (1.0 - SPINNER_ALPHA_FLOOR) * (1.0 - dist / (SPINNER_N_BARS - 1) as f64);
            let angle = (i as f64 / SPINNER_N_BARS as f64) * std::f64::consts::TAU
                - std::f64::consts::FRAC_PI_2;
            *slot = (angle, alpha);
        }
        out
    }
}

impl Default for LoadingSpinner {
    fn default() -> Self {
        Self::new()
    }
}

fn spinner_size() -> (f64, f64) {
    let diameter = 2.0 * (SPINNER_INNER_RADIUS + SPINNER_BAR_LENGTH);
    (diameter, diameter)
}

// ═══════════════════════════════════════════════════════════════════════════
//   Cairo / GTK backend (Hyprland)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(all(target_os = "linux", feature = "hyprland"))]
mod cairo_backend {
    use super::*;

    /// Anything the overlay can paint at a given position. `(x, y)` is the
    /// drawable's visual CENTER, so different-sized drawables align consistently
    /// when placed at the same coordinate.
    pub trait Drawable {
        fn draw(&self, cr: &cairo::Context, x: f64, y: f64);
        /// Width and height in display pixels.
        fn size(&self) -> (f64, f64);
    }

    /// A decoded PNG rasterised to a Cairo ARgb32 surface, ready to blit.
    pub struct Sprite {
        surface: cairo::ImageSurface,
        scale: f64,
        display_size: f64,
    }

    impl Sprite {
        /// Decode `bytes` (a PNG) and prepare a surface scaled to `display_size`
        /// display pixels square. The RGBA→BGRA premultiplied conversion happens
        /// once here, not per frame.
        pub fn from_png(bytes: &[u8], display_size: f64) -> Self {
            let img = image::load_from_memory(bytes)
                .expect("failed to decode PNG")
                .to_rgba8();
            let (w, h) = (img.width() as i32, img.height() as i32);

            let mut bgra: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
            for pixel in img.pixels() {
                let r = pixel[0] as u16;
                let g = pixel[1] as u16;
                let b = pixel[2] as u16;
                let a = pixel[3] as u16;
                bgra.push((b * a / 255) as u8);
                bgra.push((g * a / 255) as u8);
                bgra.push((r * a / 255) as u8);
                bgra.push(a as u8);
            }

            let stride = cairo::Format::ARgb32
                .stride_for_width(w as u32)
                .expect("invalid stride for ARgb32");
            let surface =
                cairo::ImageSurface::create_for_data(bgra, cairo::Format::ARgb32, w, h, stride)
                    .expect("failed to create cairo surface");

            let scale = display_size / w as f64;
            Self {
                surface,
                scale,
                display_size,
            }
        }
    }

    impl Drawable for Sprite {
        fn draw(&self, cr: &cairo::Context, x: f64, y: f64) {
            let half = self.display_size / 2.0;
            cr.save().expect("cairo save failed");
            cr.translate(x - half, y - half);
            cr.scale(self.scale, self.scale);
            cr.set_source_surface(&self.surface, 0.0_f64, 0.0_f64)
                .expect("set_source_surface failed");
            cr.paint().expect("paint failed");
            cr.restore().expect("cairo restore failed");
        }

        fn size(&self) -> (f64, f64) {
            (self.display_size, self.display_size)
        }
    }

    impl Drawable for Soundwave {
        fn draw(&self, cr: &cairo::Context, x: f64, y: f64) {
            cr.set_source_rgba(COLOR.0, COLOR.1, COLOR.2, COLOR.3);
            for (bx, by, bw, bh) in self.bars() {
                rounded_rect(cr, x + bx, y + by, bw, bh, CORNER_RADIUS);
                cr.fill().expect("fill bar");
            }
        }

        fn size(&self) -> (f64, f64) {
            soundwave_size()
        }
    }

    impl Drawable for LoadingSpinner {
        fn draw(&self, cr: &cairo::Context, x: f64, y: f64) {
            for (angle, alpha) in self.bars() {
                cr.save().expect("cairo save failed");
                cr.translate(x, y);
                cr.rotate(angle);
                cr.set_source_rgba(SPINNER_COLOR.0, SPINNER_COLOR.1, SPINNER_COLOR.2, alpha);
                rounded_rect(
                    cr,
                    SPINNER_INNER_RADIUS,
                    -SPINNER_BAR_WIDTH / 2.0,
                    SPINNER_BAR_LENGTH,
                    SPINNER_BAR_WIDTH,
                    SPINNER_CORNER_RADIUS,
                );
                cr.fill().expect("fill bar");
                cr.restore().expect("cairo restore failed");
            }
        }

        fn size(&self) -> (f64, f64) {
            spinner_size()
        }
    }

    /// Build a rounded-rect path on `cr`. Caller calls `fill` or `stroke` after.
    fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
        let r = r.min(w / 2.0).min(h / 2.0);
        let pi = std::f64::consts::PI;
        cr.new_sub_path();
        cr.arc(x + w - r, y + r, r, -pi / 2.0, 0.0);
        cr.arc(x + w - r, y + h - r, r, 0.0, pi / 2.0);
        cr.arc(x + r, y + h - r, r, pi / 2.0, pi);
        cr.arc(x + r, y + r, r, pi, 3.0 * pi / 2.0);
        cr.close_path();
    }

    /// A transparent DrawingArea that paints any `Drawable` at sub-pixel
    /// coordinates. Cairo handles fractional positioning with bilinear
    /// interpolation, so sprites glide smoothly between pixel grid cells.
    pub struct Painter {
        drawing_area: gtk::DrawingArea,
        position: Rc<Cell<(f64, f64)>>,
        drawable: Rc<RefCell<Box<dyn Drawable>>>,
    }

    impl Painter {
        pub fn new(drawable: Box<dyn Drawable>) -> Self {
            let drawing_area = gtk::DrawingArea::new();
            let position = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
            let drawable = Rc::new(RefCell::new(drawable));

            let pos = position.clone();
            let drw = drawable.clone();
            drawing_area.set_draw_func(move |_, cr, width, height| {
                let (raw_x, raw_y) = pos.get();
                let d = drw.borrow();
                let (dw, dh) = d.size();
                let half_w = dw / 2.0;
                let half_h = dh / 2.0;
                let max_x = (width as f64 - half_w).max(half_w);
                let max_y = (height as f64 - half_h).max(half_h);
                let x = raw_x.clamp(half_w, max_x);
                let y = raw_y.clamp(half_h, max_y);
                d.draw(cr, x, y);
            });

            Self {
                drawing_area,
                position,
                drawable,
            }
        }

        /// Swap the drawable at runtime; queues a redraw immediately.
        pub fn set_drawable(&self, drawable: Box<dyn Drawable>) {
            *self.drawable.borrow_mut() = drawable;
            self.drawing_area.queue_draw();
        }

        /// Move the drawable to (x, y). Sub-pixel f64 coordinates. Triggers a
        /// redraw on the next GTK frame.
        pub fn set_position(&self, x: f64, y: f64) {
            self.position.set((x, y));
            self.drawing_area.queue_draw();
        }

        /// The widget to add as the parent window's child.
        pub fn widget(&self) -> &gtk::DrawingArea {
            &self.drawing_area
        }
    }
}

#[cfg(all(target_os = "linux", feature = "hyprland"))]
pub use cairo_backend::{Painter, Sprite};

// ═══════════════════════════════════════════════════════════════════════════
//   tiny-skia backend (winit: Windows, macOS, X11)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
mod skia_backend {
    use super::*;

    /// Same role as `Drawable` but renders into a tiny_skia::Pixmap. `(x, y)`
    /// is the visual CENTER, matching the Cairo trait so positions are
    /// interchangeable.
    pub trait DrawSkia {
        fn draw_skia(&self, pixmap: &mut Pixmap, x: f64, y: f64);
        fn size(&self) -> (f64, f64);
    }

    /// A decoded PNG kept as a premultiplied tiny_skia::Pixmap, ready to blit
    /// onto a canvas via draw_pixmap.
    pub struct SpriteSkia {
        pixmap: Pixmap,
        scale: f32,
        display_size: f64,
    }

    impl SpriteSkia {
        pub fn from_png(bytes: &[u8], display_size: f64) -> Self {
            let pixmap = Pixmap::decode_png(bytes).expect("decode PNG into Pixmap");
            let scale = (display_size / pixmap.width() as f64) as f32;
            Self {
                pixmap,
                scale,
                display_size,
            }
        }
    }

    impl DrawSkia for SpriteSkia {
        fn draw_skia(&self, pixmap: &mut Pixmap, x: f64, y: f64) {
            let half = (self.display_size / 2.0) as f32;
            let transform = Transform::from_scale(self.scale, self.scale)
                .post_translate(x as f32 - half, y as f32 - half);
            pixmap.draw_pixmap(
                0,
                0,
                self.pixmap.as_ref(),
                &PixmapPaint {
                    quality: tiny_skia::FilterQuality::Bilinear,
                    ..Default::default()
                },
                transform,
                None,
            );
        }

        fn size(&self) -> (f64, f64) {
            (self.display_size, self.display_size)
        }
    }

    impl DrawSkia for Soundwave {
        fn draw_skia(&self, pixmap: &mut Pixmap, x: f64, y: f64) {
            let mut paint = Paint::default();
            paint.set_color(
                tiny_skia::Color::from_rgba(
                    COLOR.0 as f32,
                    COLOR.1 as f32,
                    COLOR.2 as f32,
                    COLOR.3 as f32,
                )
                .unwrap(),
            );
            paint.anti_alias = true;

            for (bx, by, bw, bh) in self.bars() {
                if let Some(path) = rounded_rect_path(
                    (x + bx) as f32,
                    (y + by) as f32,
                    bw as f32,
                    bh as f32,
                    CORNER_RADIUS as f32,
                ) {
                    pixmap.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }

        fn size(&self) -> (f64, f64) {
            soundwave_size()
        }
    }

    impl DrawSkia for LoadingSpinner {
        fn draw_skia(&self, pixmap: &mut Pixmap, x: f64, y: f64) {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };

            // One unit bar at (SPINNER_INNER_RADIUS, -SPINNER_BAR_WIDTH/2) in
            // local space, before rotation. Build it once and re-transform per bar.
            let bar_path = match rounded_rect_path(
                SPINNER_INNER_RADIUS as f32,
                -(SPINNER_BAR_WIDTH as f32) / 2.0,
                SPINNER_BAR_LENGTH as f32,
                SPINNER_BAR_WIDTH as f32,
                SPINNER_CORNER_RADIUS as f32,
            ) {
                Some(p) => p,
                None => return,
            };

            for (angle, alpha) in self.bars() {
                paint.set_color(
                    tiny_skia::Color::from_rgba(
                        SPINNER_COLOR.0 as f32,
                        SPINNER_COLOR.1 as f32,
                        SPINNER_COLOR.2 as f32,
                        alpha as f32,
                    )
                    .unwrap(),
                );
                let transform = Transform::from_rotate(angle.to_degrees() as f32)
                    .post_translate(x as f32, y as f32);
                pixmap.fill_path(&bar_path, &paint, FillRule::Winding, transform, None);
            }
        }

        fn size(&self) -> (f64, f64) {
            spinner_size()
        }
    }

    /// Build a rounded-rect path using cubic Bezier corners. Returns None if
    /// the rectangle has zero area (PathBuilder rejects empty paths).
    /// KAPPA ≈ 4*(sqrt(2)-1)/3 is the standard control-point ratio for
    /// approximating a quarter circle with a cubic.
    fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let r = r.min(w / 2.0).min(h / 2.0);
        const KAPPA: f32 = 0.552_284_8;
        let c = r * KAPPA;
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
        pb.close();
        pb.finish()
    }
}

#[cfg(not(all(target_os = "linux", feature = "hyprland")))]
pub use skia_backend::{DrawSkia, SpriteSkia};
