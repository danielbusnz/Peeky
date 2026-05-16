// Standalone visual test for a soundwave / equalizer-style listening animation.
//
// Renders N vertical bars that bob up and down at slightly different
// frequencies, so they look like a live audio visualizer even though
// nothing is actually plugged into the mic. Tune the constants and re-run.
//
// Run with: cargo run --bin test_soundwave --features hyprland

use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, glib};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const APP_ID: &str = "com.aegis.test-soundwave";
const WIN_W: i32 = 320;
const WIN_H: i32 = 240;

// ── Animation parameters — twist these ───────────────────────────────────────

/// Number of bars in the visualizer.
const N_BARS: usize = 5;
/// Width of each bar in pixels.
const BAR_WIDTH: f64 = 10.0;
/// Gap between bars in pixels.
const BAR_GAP: f64 = 8.0;
/// Minimum bar height at the trough of its sine wave.
const MIN_HEIGHT: f64 = 14.0;
/// Maximum bar height at the peak of its sine wave.
const MAX_HEIGHT: f64 = 90.0;
/// Base pulse rate in Hz. Each bar gets this plus a small per-bar offset
/// so the bars de-phase and look organic.
const BASE_FREQUENCY_HZ: f64 = 1.6;
/// Per-bar frequency spread. Bar i runs at BASE + i * SPREAD Hz.
const FREQUENCY_SPREAD: f64 = 0.35;
/// Bar color (r, g, b, a). Each 0.0–1.0.
const COLOR: (f64, f64, f64, f64) = (0.30, 0.70, 1.00, 0.95);
/// Background color so the bars stand out while testing.
const BG: (f64, f64, f64) = (0.08, 0.08, 0.12);
/// Corner radius for rounded bar caps. 0.0 = sharp rectangles.
const CORNER_RADIUS: f64 = 4.0;

// ─────────────────────────────────────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("soundwave test")
        .default_width(WIN_W)
        .default_height(WIN_H)
        .build();

    let drawing_area = gtk::DrawingArea::new();
    let start = Rc::new(Cell::new(Instant::now()));

    let start_for_draw = start.clone();
    drawing_area.set_draw_func(move |_, cr, width, height| {
        let t = start_for_draw.get().elapsed().as_secs_f64();

        // Background.
        cr.set_source_rgb(BG.0, BG.1, BG.2);
        cr.paint().expect("paint bg");

        // Center the group of bars horizontally.
        let total_width = N_BARS as f64 * BAR_WIDTH + (N_BARS as f64 - 1.0) * BAR_GAP;
        let start_x = (width as f64 - total_width) / 2.0;
        let center_y = height as f64 / 2.0;

        cr.set_source_rgba(COLOR.0, COLOR.1, COLOR.2, COLOR.3);

        for i in 0..N_BARS {
            // Each bar bobs on its own sine wave. Different frequencies
            // and starting phases so they look independent.
            let freq = BASE_FREQUENCY_HZ + i as f64 * FREQUENCY_SPREAD;
            let phase = i as f64 * 0.7;
            let s = (t * freq * std::f64::consts::TAU + phase).sin();
            // sin returns -1..=1, map to 0..=1 for height interpolation.
            let unit = (s + 1.0) / 2.0;
            let bar_h = MIN_HEIGHT + (MAX_HEIGHT - MIN_HEIGHT) * unit;

            let x = start_x + i as f64 * (BAR_WIDTH + BAR_GAP);
            let y = center_y - bar_h / 2.0;
            rounded_rect(cr, x, y, BAR_WIDTH, bar_h, CORNER_RADIUS);
            cr.fill().expect("fill bar");
        }
    });

    window.set_child(Some(&drawing_area));
    window.present();

    let drawing_area_for_tick = drawing_area.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        drawing_area_for_tick.queue_draw();
        glib::ControlFlow::Continue
    });
}

/// Build a rounded-rect path on `cr`. Caller calls `fill` or `stroke` after.
/// Cairo doesn't have a native rounded rect, so this stitches one out of
/// four quarter-arcs at the corners.
fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    // Clamp the radius so it never exceeds half the smaller dimension,
    // otherwise the arcs overlap and the shape gets weird.
    let r = r.min(w / 2.0).min(h / 2.0);
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -pi / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, pi / 2.0);
    cr.arc(x + r, y + h - r, r, pi / 2.0, pi);
    cr.arc(x + r, y + r, r, pi, 3.0 * pi / 2.0);
    cr.close_path();
}
