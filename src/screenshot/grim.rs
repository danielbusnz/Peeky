use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hyprland::data::{Monitors, Workspace};
use hyprland::shared::{HyprData, HyprDataActive};
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use std::io::Cursor;
use std::process::Command;

/// Captures the entire monitor that's showing the currently active workspace.
pub fn capture_active_workspace() -> Result<(String, u32, u32), Box<dyn std::error::Error>> {
    let active = Workspace::get_active()?;
    let monitor = Monitors::get()?
        .into_iter()
        .find(|m| m.active_workspace.id == active.id)
        .ok_or("no monitor for active workspace")?;

    capture_for_claude(
        monitor.x,
        monitor.y,
        monitor.width as i32,
        monitor.height as i32,
    )
}

/// Returns the geometry (x, y, width, height) of the monitor showing the
/// currently active workspace, without capturing a screenshot. Used by
/// callers that delegate the capture step (e.g., `detect_element_location`).
pub fn active_workspace_geometry()
-> Result<(i32, i32, u32, u32), Box<dyn std::error::Error>> {
    let active = Workspace::get_active()?;
    let monitor = Monitors::get()?
        .into_iter()
        .find(|m| m.active_workspace.id == active.id)
        .ok_or("no monitor for active workspace")?;
    Ok((
        monitor.x,
        monitor.y,
        monitor.width as u32,
        monitor.height as u32,
    ))
}

/// Captures a screen region with grim, encodes as JPEG q85, and returns base64
/// plus the captured dimensions. Resize-to-Computer-Use is commented out for
/// now — image is returned at native resolution.
pub fn capture_for_claude(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(String, u32, u32), Box<dyn std::error::Error>> {
    let geometry = format!("{},{} {}x{}", x, y, width, height);
    let output = Command::new("grim").args(["-g", &geometry, "-"]).output()?;

    if !output.status.success() {
        return Err(format!("grim failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let img = image::load_from_memory(&output.stdout)?;
    // let img = img.resize_exact(declared_w, declared_h, FilterType::Lanczos3);

    let mut jpeg: Vec<u8> = Vec::new();
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, 85))?;
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
/// with Lanczos3 filtering, and re-encode as JPEG q85. Used by Computer Use
/// callers so Claude's returned coordinates can be scaled back accurately.
/// Ported from Tabby.
pub fn resize_jpeg_for_computer_use(
    src_b64: &str,
    target_w: u32,
    target_h: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = BASE64.decode(src_b64.as_bytes())?;
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;
    let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);

    let mut out: Vec<u8> = Vec::new();
    {
        let encoder = JpegEncoder::new_with_quality(&mut out, 85);
        resized.write_with_encoder(encoder)?;
    }
    Ok(BASE64.encode(&out))
}
