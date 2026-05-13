use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use std::io::Cursor;

/// Returns the geometry (x, y, width, height) of the primary monitor.
/// Cross-platform fallback: there's no consistent "active workspace" concept
/// across Mac/Windows/Linux, so we use the primary monitor.
pub fn active_workspace_geometry()
-> Result<(i32, i32, u32, u32), Box<dyn std::error::Error>> {
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

/// Decode an existing base64 JPEG, resize to exactly the declared dimensions
/// with Triangle filtering, and re-encode as JPEG q85.
pub fn resize_jpeg_for_computer_use(
    src_b64: &str,
    target_w: u32,
    target_h: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = BASE64.decode(src_b64.as_bytes())?;
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;
    let resized = img.resize_exact(target_w, target_h, FilterType::Triangle);

    let mut out: Vec<u8> = Vec::new();
    {
        let encoder = JpegEncoder::new_with_quality(&mut out, 85);
        resized.write_with_encoder(encoder)?;
    }
    Ok(BASE64.encode(&out))
}
