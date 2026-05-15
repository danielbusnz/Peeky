use gtk::gdk::Display;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, CssProvider, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::painter::Painter;

const APP_ID: &str = "com.tabby.cursor-mvp";
// Time for cursor lag to halve. 91.7ms reproduces the previous 500Hz × 0.015
// feel under a delta-time formulation, so changing TICK_MS no longer alters
// the perceived snappiness.
const SMOOTHING_HALF_LIFE: f64 = 0.0917;
const TICK_MS: u64 = 2;
const Y_OFFSET: i32 = -50;
const X_OFFSET: i32 = 10;
const POINT_DURATION: Duration = Duration::from_secs(3);

static CURSOR_SENDER: OnceLock<Sender<(i32, i32)>> = OnceLock::new();

/// Ask the cursor to fly to (x, y) and sit there for ~3 seconds, then resume
/// following the mouse. Callable from any thread. No-op if `cursor()` hasn't
/// been initialized yet.
pub fn point_at(x: i32, y: i32) {
    if let Some(sender) = CURSOR_SENDER.get() {
        let _ = sender.send((x, y));
    }
}

pub fn cursor(x: i32, y: i32) -> glib::ExitCode {
    let (sender, receiver) = channel();
    let _ = CURSOR_SENDER.set(sender);
    let receiver_holder = RefCell::new(Some(receiver));

    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(install_css);
    app.connect_activate(move |app| {
        let receiver = receiver_holder
            .borrow_mut()
            .take()
            .expect("connect_activate fired more than once");
        let window = build_window(app);
        let painter = Painter::new();
        window.set_child(Some(painter.widget()));
        make_click_through(&window);
        window.present();
        println!("[gtk] cursor window presented");
        start_tracking(painter, x, y, receiver);
    });
    app.run()
}

fn install_css(_app: &Application) {
    let provider = CssProvider::new();
    provider.load_from_data("window { background: transparent; }");
    gtk::style_context_add_provider_for_display(
        &Display::default().expect("could not connect to a display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn build_window(app: &Application) -> ApplicationWindow {
    let window = ApplicationWindow::builder().application(app).build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    // Anchor all four edges so the window fills the entire screen. The
    // cursor is then drawn at sub-pixel coords *inside* this canvas.
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, true);
    window
}

fn make_click_through(window: &ApplicationWindow) {
    window.connect_realize(|window| {
        if let Some(surface) = window.surface() {
            let empty_region = gtk::cairo::Region::create();
            surface.set_input_region(Some(&empty_region));
        }
    });
}

fn start_tracking(
    painter: Painter,
    initial_x: i32,
    initial_y: i32,
    receiver: Receiver<(i32, i32)>,
) {
    let mut cursor_x = initial_x as f64;
    let mut cursor_y = initial_y as f64;
    let mut override_target: Option<(i32, i32, Instant)> = None;
    let mut last_tick: Option<Instant> = None;

    glib::timeout_add_local(Duration::from_millis(TICK_MS), move || {
        let now = Instant::now();
        let delta_t = match last_tick {
            Some(prev) => now.duration_since(prev).as_secs_f64(),
            None => 0.0,
        };
        last_tick = Some(now);

        // Drain any pending point_at commands; the latest one wins.
        while let Ok((target_x, target_y)) = receiver.try_recv() {
            override_target = Some((target_x, target_y, Instant::now() + POINT_DURATION));
        }

        // Pick the target + whether to apply the floating-above offsets.
        // When pointing (override active), draw EXACTLY on the target so
        // Claude's coordinates land where they should. When following the
        // mouse, apply the usual offsets so the sprite floats next to the
        // pointer instead of obscuring it.
        let (target, apply_offsets) = match override_target {
            Some((target_x, target_y, until)) if Instant::now() < until => {
                (Some((target_x as f64, target_y as f64)), false)
            }
            _ => {
                override_target = None;
                let mouse = crate::mouse::mouse_movement()
                    .ok()
                    .map(|(mouse_x, mouse_y)| (mouse_x as f64, mouse_y as f64));
                (mouse, true)
            }
        };

        if let Some((target_x, target_y)) = target {
            let alpha = 1.0 - 2f64.powf(-delta_t / SMOOTHING_HALF_LIFE);
            cursor_x += (target_x - cursor_x) * alpha;
            cursor_y += (target_y - cursor_y) * alpha;
            let (ox, oy) = if apply_offsets {
                (X_OFFSET as f64, Y_OFFSET as f64)
            } else {
                (0.0, 0.0)
            };
            painter.set_position(cursor_x + ox, cursor_y + oy);
        }

        glib::ControlFlow::Continue
    });
}
