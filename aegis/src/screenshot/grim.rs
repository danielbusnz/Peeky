use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hyprland::data::{Monitors, Workspace};
use hyprland::shared::{HyprData, HyprDataActive};
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use std::io::Cursor;
use std::process::Command;

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

/// Capture a screen region, decode it, resize to (target_w, target_h) with
/// SIMD bilinear filtering, encode as JPEG q85, base64. ONE decode and ONE
/// encode end-to-end. This is the fast path the agent loop uses every
/// iteration.
///
/// `-t jpeg` tells grim to output JPEG bytes directly (faster decode than
/// its default PNG). The resize then bilinear-downsamples to the declared
/// Computer Use resolution.
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

    let geometry = format!("{},{} {}x{}", x, y, width, height);
    let output = Command::new("grim")
        .args(["-t", "jpeg", "-g", &geometry, "-"])
        .output()?;
    if !output.status.success() {
        return Err(format!("grim failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    let src_dyn = ImageReader::new(Cursor::new(output.stdout))
        .with_guessed_format()?
        .decode()?;
    let src_rgb = src_dyn.to_rgb8();
    let (src_w, src_h) = (src_rgb.width(), src_rgb.height());

    let fir_src = FirImage::from_vec_u8(src_w, src_h, src_rgb.into_raw(), PixelType::U8x3)?;
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

