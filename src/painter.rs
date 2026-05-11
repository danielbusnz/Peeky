use gtk::cairo;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const CURSOR_PNG: &[u8] = include_bytes!("../assets/cursor.png");
const CURSOR_DISPLAY_SIZE: f64 = 18.0;

/// A transparent DrawingArea that paints the cursor PNG at sub-pixel
/// coordinates. Cairo handles the fractional positioning with bilinear
/// interpolation, so the cursor glides smoothly between pixel grid cells
/// instead of snapping to whole-pixel positions.
pub struct Painter {
    drawing_area: gtk::DrawingArea,
    position: Rc<Cell<(f64, f64)>>,
}

impl Painter {
    pub fn new() -> Self {
        let drawing_area = gtk::DrawingArea::new();
        let position = Rc::new(Cell::new((0.0, 0.0)));

        let surface = build_surface();
        let scale = CURSOR_DISPLAY_SIZE / surface.width() as f64;

        let pos = position.clone();
        drawing_area.set_draw_func(move |_, cr, _, _| {
            let (x, y) = pos.get();
            cr.save().expect("cairo save failed");
            cr.translate(x, y);
            cr.scale(scale, scale);
            cr.set_source_surface(&surface, 0.0, 0.0)
                .expect("set_source_surface failed");
            cr.paint().expect("paint failed");
            cr.restore().expect("cairo restore failed");
        });

        Self {
            drawing_area,
            position,
        }
    }

    /// Move the cursor to (x, y). Sub-pixel f64 coordinates. Triggers a
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

/// Decode cursor.png with the `image` crate, then convert RGBA (straight
/// alpha) to ARgb32 (BGRA premultiplied, native byte order) which is what
/// Cairo expects.
fn build_surface() -> cairo::ImageSurface {
    let img = image::load_from_memory(CURSOR_PNG)
        .expect("failed to decode cursor.png")
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
    cairo::ImageSurface::create_for_data(bgra, cairo::Format::ARgb32, w, h, stride)
        .expect("failed to create cairo surface")
}
