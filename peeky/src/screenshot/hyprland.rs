use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hyprland::data::{Monitors, Workspace};
use hyprland::shared::{HyprData, HyprDataActive};
use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use std::io::Cursor;
use std::process::Command;

use super::backend::ScreenshotBackend;

/// Hyprland + grim backend. Zero-sized; never instantiated.
pub struct Backend;

impl ScreenshotBackend for Backend {
    /// Geometry of the monitor showing the currently active workspace, via
    /// Hyprland IPC, without capturing a screenshot.
    fn active_workspace_geometry()
    -> Result<(i32, i32, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
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

    /// `-t jpeg` tells grim to output JPEG bytes directly (faster decode than
    /// its default PNG). The resize then bilinear-downsamples to the declared
    /// Computer Use resolution.
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
}
