//! Windows input injection: not yet implemented. Pointing, opening URLs, and
//! launching apps still work; only synthesized click/type/key/scroll is
//! unavailable until this is built out (e.g. via the `windows` crate's
//! `SendInput`).

use super::backend::InputInjector;

/// Windows backend stub. Zero-sized; never instantiated.
pub struct Backend;

impl InputInjector for Backend {
    fn exec_click(_x: i64, _y: i64) {
        eprintln!("[action:click] input injection not implemented on Windows");
    }

    fn exec_type(_text: &str) {
        eprintln!("[action:type] input injection not implemented on Windows");
    }

    fn exec_key(_combo: &str) {
        eprintln!("[action:key] input injection not implemented on Windows");
    }

    fn exec_scroll(_direction: &str, _amount: u32) {
        eprintln!("[action:scroll] input injection not implemented on Windows");
    }

    fn check_available() {
        eprintln!(
            "[startup] WARNING: input injection is not implemented on Windows. Click actions \
             will move the overlay but NOT inject real input."
        );
    }
}
