use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hyprland::data::{Monitors, Workspace};
use hyprland::shared::{HyprData, HyprDataActive};
use image::codecs::jpeg::JpegEncoder;
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
