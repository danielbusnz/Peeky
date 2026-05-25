use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::codecs::jpeg::JpegEncoder;

/// Returns the geometry (x, y, width, height) of the primary monitor.
/// Cross-platform fallback: there's no consistent "active workspace" concept
/// across Mac/Windows/Linux, so we use the primary monitor.
pub fn active_workspace_geometry() -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error>> {
    let monitors = ::xcap::Monitor::all()?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or("no primary monitor")?;
    Ok((
        monitor.x()?,
        monitor.y()?,
        monitor.width()?,
        monitor.height()?,
    ))
}

/// Captures a screen region via xcap, encodes as JPEG q85, returns base64
/// plus the captured dimensions.
///
/// Only the demo binary calls this directly; the aegis hot path uses
/// `capture_resized_for_claude`. Kept here so the demo and the main
/// pipeline share one screenshot module.
#[allow(dead_code)]
pub fn capture_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(String, u32, u32), Box<dyn std::error::Error>> {
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

/// Fast path: capture + resize + encode in one call. Matches the grim
/// backend's signature so callers can be backend-agnostic.
pub fn capture_resized_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    target_w: u32,
    target_h: u32,
) -> Result<String, Box<dyn std::error::Error>> {
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

/// Pick one of three aspect-matched resolutions Anthropic recommends for
/// Computer Use. Matching the input's aspect ratio avoids stretching that
/// degrades coordinate accuracy. Ported from Tabby.
pub fn pick_declared_resolution(window_width: i64, window_height: i64) -> (u32, u32) {
    let ratio = window_width as f64 / window_height.max(1) as f64;
    let candidates: [(u32, u32, f64); 3] = [
        (1024, 768, 4.0 / 3.0),
        (1280, 800, 16.0 / 10.0),
        (1366, 768, 16.0 / 9.0),
    ];
    let mut best = candidates[1];
    let mut smallest_diff = f64::INFINITY;
    for (w, h, ar) in candidates {
        let diff = (ratio - ar).abs();
        if diff < smallest_diff {
            smallest_diff = diff;
            best = (w, h, ar);
        }
    }
    (best.0, best.1)
}
