use std::sync::Once;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::codecs::jpeg::JpegEncoder;

use super::backend::ScreenshotBackend;

/// Logs the monitor geometry diagnostic at most once per process.
static GEOMETRY_DIAG: Once = Once::new();

/// Cross-platform backend (Windows, macOS, X11) via the `xcap` crate.
/// Zero-sized; never instantiated.
pub struct Backend;

impl ScreenshotBackend for Backend {
    /// Geometry of the primary monitor. There is no consistent "active
    /// workspace" concept across Mac/Windows/Linux, so we use the primary.
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let monitors = ::xcap::Monitor::all()?;
        let monitor = monitors
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or("no primary monitor")?;
        let geo = (
            monitor.x()?,
            monitor.y()?,
            monitor.width()?,
            monitor.height()?,
        );

        // DIAGNOSTIC (once per process): find_action maps Claude's coordinates
        // into these w/h units, then clicks via CGEvent, which uses logical
        // points on macOS. So these must be logical. If `w`/`h` come back as the
        // physical (Retina-doubled) resolution, find_action clicks land ~2x off
        // and the mapping needs dividing by scale_factor. On a Retina Mac: if w
        // ≈ your logical width it's correct; if ≈ 2x it's physical.
        GEOMETRY_DIAG.call_once(|| {
            let sf = monitor.scale_factor().unwrap_or(1.0);
            let (_, _, w, h) = geo;
            eprintln!(
                "[diag:geometry] xcap primary monitor: pos=({}, {}) size={}x{} \
                 scale_factor={:.2} → physical would be {}x{}. \
                 Clicks use logical points; size above must be logical.",
                geo.0,
                geo.1,
                w,
                h,
                sf,
                (w as f32 * sf) as u32,
                (h as f32 * sf) as u32,
            );
        });

        Ok(geo)
    }

    fn capture_resized_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        target_w: u32,
        target_h: u32,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use fast_image_resize::images::Image as FirImage;
        use fast_image_resize::{
            FilterType as FirFilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        };

        let monitor = ::xcap::Monitor::from_point(x, y)?;
        let local_x = (x - monitor.x()?).max(0) as u32;
        let local_y = (y - monitor.y()?).max(0) as u32;
        let image = monitor.capture_region(local_x, local_y, width as u32, height as u32)?;

        // On macOS Retina displays, capture_region returns physical pixels even
        // though we pass logical points. Use the actual image dimensions for
        // resize so the coordinate mapping stays correct.
        let src_w = image.width();
        let src_h = image.height();

        // xcap gives us an RGBA buffer directly. Convert to RGB for the resize.
        let rgba = image.into_raw();
        let mut rgb: Vec<u8> = Vec::with_capacity((src_w * src_h * 3) as usize);
        for chunk in rgba.chunks_exact(4) {
            rgb.push(chunk[0]);
            rgb.push(chunk[1]);
            rgb.push(chunk[2]);
        }

        let fir_src = FirImage::from_vec_u8(src_w, src_h, rgb, PixelType::U8x3)?;
        let mut fir_dst = FirImage::new(target_w, target_h, PixelType::U8x3);
        let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilterType::Bilinear));
        let mut resizer = Resizer::new();
        resizer.resize(&fir_src, &mut fir_dst, &opts)?;

        let mut out: Vec<u8> = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85).encode(
            fir_dst.buffer(),
            target_w,
            target_h,
            image::ExtendedColorType::Rgb8,
        )?;
        Ok(BASE64.encode(&out))
    }
}

impl Backend {
    /// Capture a region, encode as JPEG q85, return base64 plus captured
    /// dimensions. Not part of the `ScreenshotBackend` contract: only the
    /// `demo_win` binary calls it, and grim has no equivalent. Kept here so
    /// the demo and the main pipeline share one screenshot module.
    #[allow(dead_code)]
    pub fn capture_for_claude(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<(String, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let monitor = ::xcap::Monitor::from_point(x, y)?;
        // capture_region takes monitor-local coords, so subtract monitor origin.
        let local_x = (x - monitor.x()?).max(0) as u32;
        let local_y = (y - monitor.y()?).max(0) as u32;
        let image = monitor.capture_region(local_x, local_y, width as u32, height as u32)?;

        let mut jpeg: Vec<u8> = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut jpeg, 85);
        image.write_with_encoder(encoder)?;
        Ok((BASE64.encode(&jpeg), width as u32, height as u32))
    }
}
