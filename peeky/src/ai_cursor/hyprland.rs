//! Hyprland cursor overlay. A GTK4 layer-shell window draws peeky's
//! sprite on top of every other window, click-through so the user's
//! input still reaches the app underneath. The window covers the whole
//! monitor; the sprite gets drawn at sub-pixel coords inside.
//!
//! Two state sources update the sprite each tick: the live system mouse
//! position (default behavior) and explicit `point_at` overrides from
//! Claude (cursor flies to a coordinate, sits for ~3s, then resumes
//! following the mouse).

use gtk::gdk::Display;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, CssProvider, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use super::CursorState;
use crate::painter::{LoadingSpinner, Painter, Soundwave, Sprite};

const APP_ID: &str = "com.tabby.cursor-mvp";
const CURSOR_PNG: &[u8] = include_bytes!("../../assets/cursor.png");
const CURSOR_DISPLAY_SIZE: f64 = 18.0;
// Time for cursor lag to halve. 91.7ms reproduces the previous 500Hz × 0.015
// feel under a delta-time formulation, so changing TICK_MS no longer alters
// the perceived snappiness.
const SMOOTHING_HALF_LIFE: f64 = 0.0917;
const TICK_MS: u64 = 2;
const Y_OFFSET: i32 = -70;
const X_OFFSET: i32 = 20;
const POINT_DURATION: Duration = Duration::from_secs(3);

/// Channel for `point_at` calls. Initialized inside `cursor()` so any
/// thread can ask the GTK thread to fly the sprite without sharing a
/// `!Send` GTK reference.
static CURSOR_SENDER: OnceLock<Sender<(i32, i32)>> = OnceLock::new();

/// Channel for cursor-state transitions (Idle/Listening/Loading).
/// Same shape as CURSOR_SENDER and same reason.
static STATE_SENDER: OnceLock<Sender<CursorState>> = OnceLock::new();

/// Push a CursorState transition onto the GTK thread. Callable from any
/// thread. No-op until `cursor()` has installed the receiver.
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

/// Initialize and run the cursor overlay. Blocks the calling thread for
/// the rest of the process (GTK's main loop). `(x, y)` is the initial
/// sprite position in absolute screen pixels.
pub fn cursor(x: i32, y: i32) -> glib::ExitCode {
    let (sender, receiver) = channel();
    let _ = CURSOR_SENDER.set(sender);
    let receiver_holder = RefCell::new(Some(receiver));

    let (state_sender, state_receiver) = channel();
    let _ = STATE_SENDER.set(state_sender);
    let state_receiver_holder = RefCell::new(Some(state_receiver));

    // Wire signal events → state channel. These run on the signal-handler
    // thread; the channel makes them safe to consume on the GTK thread.
    crate::hotkey::on_press(|| set_state(CursorState::Listening));
    crate::hotkey::on_release(|| set_state(CursorState::Loading));

    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(install_css);
    app.connect_activate(move |app| {
        let receiver = receiver_holder
            .borrow_mut()
            .take()
            .expect("connect_activate fired more than once");
        let state_receiver = state_receiver_holder
            .borrow_mut()
            .take()
            .expect("connect_activate fired more than once");
        let window = build_window(app);
        let painter = Painter::new(Box::new(Sprite::from_png(
            CURSOR_PNG,
            CURSOR_DISPLAY_SIZE * crate::painter::overlay_scale(),
        )));
        window.set_child(Some(painter.widget()));
        make_click_through(&window);
        window.present();
        println!("[gtk] cursor window presented");
        start_tracking(painter, x, y, receiver, state_receiver);
    });
    app.run()
}

/// Make the window background fully transparent so only the sprite is
/// visible. The default GTK background would obscure everything below.
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

/// Set an empty input region on the layer-shell surface so pointer
/// events pass through to whatever is underneath. Without this the
/// overlay would steal every click.
fn make_click_through(window: &ApplicationWindow) {
    window.connect_realize(|window| {
        if let Some(surface) = window.surface() {
            let empty_region = gtk::cairo::Region::create();
            surface.set_input_region(Some(&empty_region));
        }
    });
}

/// Install the per-tick callback that advances the sprite toward its
/// target. Runs at TICK_MS cadence on the GTK thread. Reads cursor
/// position via Hyprland IPC and applies an exponential-smoothing
/// interpolation; the cursor visibly follows the mouse with a small
/// delay tuned by SMOOTHING_HALF_LIFE.
fn start_tracking(
    painter: Painter,
    initial_x: i32,
    initial_y: i32,
    receiver: Receiver<(i32, i32)>,
    state_receiver: Receiver<CursorState>,
) {
    // Hyprland reports cursor coords in global virtual-desktop space, but
    // our layer-shell window's coordinate space is local to the monitor it
    // lives on (starts at 0,0 at that monitor's top-left). Subtract the
    // monitor origin so the sprite lands where the mouse actually is.
    let (mon_x, mon_y) = match crate::screenshot::active_workspace_geometry() {
        Ok((x, y, _, _)) => (x as f64, y as f64),
        Err(_) => (0.0, 0.0),
    };

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

        // Drain state changes first; the latest one wins.
        while let Ok(state) = state_receiver.try_recv() {
            match state {
                CursorState::Idle => painter.set_drawable(Box::new(Sprite::from_png(
                    CURSOR_PNG,
                    CURSOR_DISPLAY_SIZE * crate::painter::overlay_scale(),
                ))),
                CursorState::Listening => {
                    painter.set_drawable(Box::new(Soundwave::new()));
                }
                CursorState::Loading => painter.set_drawable(Box::new(LoadingSpinner::new())),
            }
        }

        // Drain any pending point_at commands; the latest one wins.
        while let Ok((target_x, target_y)) = receiver.try_recv() {
            override_target = Some((target_x, target_y, Instant::now() + POINT_DURATION));
        }

        // Pick the target + whether to apply the floating-above offsets.
        // When pointing (override active), draw EXACTLY on the target so
        // Claude's coordinates land where they should. When following the
        // mouse, apply the usual offsets so the sprite floats next to the
        // pointer instead of obscuring it. Both the override coords (from
        // Claude, scaled against a grim-captured screenshot) and the live
        // mouse coords come in global desktop space, so both get the
        // monitor-origin subtraction.
        let (target, apply_offsets) = match override_target {
            Some((target_x, target_y, until)) if Instant::now() < until => (
                Some((target_x as f64 - mon_x, target_y as f64 - mon_y)),
                false,
            ),
            _ => {
                override_target = None;
                let mouse = crate::mouse_position::mouse_movement()
                    .ok()
                    .map(|(mx, my)| (mx as f64 - mon_x, my as f64 - mon_y));
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
