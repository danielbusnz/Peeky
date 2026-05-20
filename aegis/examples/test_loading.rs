// Standalone visual test for a loading-state animation.
// Classic iOS-style radial spinner: N bars arranged in a circle, each fading
// based on its position behind a rotating "head" bar. The head sweeps around
// once per cycle, leaving a comet-style fade trail behind it.
//
// Run with: cargo run --bin test_loading --features hyprland

use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, glib};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const APP_ID: &str = "com.aegis.test-loading";
const WIN_W: i32 = 240;
const WIN_H: i32 = 240;

// ── Animation parameters ─────────────────────────────────────────────────────

/// Number of bars around the circle. 12 is the iOS / macOS standard.
const N_BARS: usize = 12;
/// Distance from center to the inner end of each bar.
const INNER_RADIUS: f64 = 8.0;
/// Length of each bar in the radial direction.
const BAR_LENGTH: f64 = 7.0;
/// Width of each bar (perpendicular to its radial direction).
const BAR_WIDTH: f64 = 2.5;
/// Full rotations per second. 1.0 = head sweeps the circle once a second.
const ROTATION_HZ: f64 = 1.0;
/// Minimum alpha for the dimmest bar (the one most "behind" the head).
const ALPHA_FLOOR: f64 = 0.12;
/// Bar color (r, g, b). Alpha is computed per-bar below.
const COLOR: (f64, f64, f64) = (1.00, 0.55, 0.00);
/// Background color so the spinner stands out while testing.
const BG: (f64, f64, f64) = (0.08, 0.08, 0.12);
/// Corner radius for bar caps. BAR_WIDTH / 2.0 = capsule.
const CORNER_RADIUS: f64 = 1.25;

// ─────────────────────────────────────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("loading test")
        .default_width(WIN_W)
        .default_height(WIN_H)
        .build();

    let drawing_area = gtk::DrawingArea::new();
    let start = Rc::new(Cell::new(Instant::now()));

    let start_for_draw = start.clone();
    drawing_area.set_draw_func(move |_, cr, width, height| {
        let t = start_for_draw.get().elapsed().as_secs_f64();

        cr.set_source_rgb(BG.0, BG.1, BG.2);
        cr.paint().expect("paint bg");

        let cx = width as f64 / 2.0;
        let cy = height as f64 / 2.0;

        // The head's position cycles through bar indices [0, N_BARS) over time.
        // ROTATION_HZ * N_BARS gives bar-positions-per-second.
        let head = (t * ROTATION_HZ * N_BARS as f64) % N_BARS as f64;

        for i in 0..N_BARS {
            // Distance behind the head, wrapping around the circle.
            // i == head → 0 (brightest). i one step behind → 1. etc.
            let dist = (head - i as f64).rem_euclid(N_BARS as f64);
            // Linear fade: 1.0 at head, ALPHA_FLOOR at the bar farthest behind.
            let alpha =
                ALPHA_FLOOR + (1.0 - ALPHA_FLOOR) * (1.0 - dist / (N_BARS - 1) as f64);

            // Position each bar at its angle around the circle. Subtract π/2
            // so bar 0 sits at 12 o'clock instead of 3 o'clock.
            let angle = (i as f64 / N_BARS as f64) * std::f64::consts::TAU
                - std::f64::consts::FRAC_PI_2;

            cr.save().expect("save");
            cr.translate(cx, cy);
            cr.rotate(angle);
            cr.set_source_rgba(COLOR.0, COLOR.1, COLOR.2, alpha);
            rounded_rect(
                cr,
                INNER_RADIUS,
                -BAR_WIDTH / 2.0,
                BAR_LENGTH,
                BAR_WIDTH,
                CORNER_RADIUS,
            );
            cr.fill().expect("fill bar");
            cr.restore().expect("restore");
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

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -pi / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, pi / 2.0);
    cr.arc(x + r, y + h - r, r, pi / 2.0, pi);
    cr.arc(x + r, y + r, r, pi, 3.0 * pi / 2.0);
    cr.close_path();
}
