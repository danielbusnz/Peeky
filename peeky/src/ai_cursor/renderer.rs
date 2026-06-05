//! Cross-platform renderer abstraction for the cursor overlay.
//!
//! The cursor draws into a `tiny_skia::Pixmap` each frame, then hands that
//! pixmap to a `Renderer` to put on screen. Two backends:
//!
//!   - Softbuffer: every platform except macOS. CPU pixel-blit, very small
//!     dep footprint, works fine where the OS compositor honors per-pixel
//!     alpha in the buffer.
//!   - wgpu: macOS only. softbuffer 0.4 hardcodes
//!     `CGImageAlphaInfo::NoneSkipFirst` on macOS which strips alpha at the
//!     CG level before the compositor sees it. wgpu's
//!     `CompositeAlphaMode::PostMultiplied` honors per-pixel alpha.
//!
//! The dispatch is cfg-gated so each platform pays for only its backend.

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::window::Window;

use super::common::DirtyRect;

#[cfg(target_os = "macos")]
use super::macos::WgpuRenderer;

pub enum Renderer {
    #[cfg(not(target_os = "macos"))]
    Softbuffer(SoftbufferRenderer),
    #[cfg(target_os = "macos")]
    Wgpu(WgpuRenderer),
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self::Softbuffer(SoftbufferRenderer::new(window)?))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(Self::Wgpu(WgpuRenderer::new(window)?))
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::Softbuffer(r) => r.resize(width, height),
            #[cfg(target_os = "macos")]
            Self::Wgpu(r) => {
                r.resize(width, height);
                Ok(())
            }
        }
    }

    /// Present the current canvas to the window. `dirty` is the bounding
    /// box of pixels that changed this frame (or None when nothing changed
    /// and we're just keeping the swapchain alive). Softbuffer uses the
    /// dirty rect to skip work; wgpu uploads the whole canvas regardless.
    pub fn present(&mut self, canvas: &Pixmap, dirty: Option<DirtyRect>) -> Result<(), String> {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::Softbuffer(r) => r.present(canvas, dirty),
            #[cfg(target_os = "macos")]
            Self::Wgpu(r) => {
                let _ = dirty;
                r.present(canvas)
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Softbuffer (everywhere except macOS)
// ────────────────────────────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
pub struct SoftbufferRenderer {
    _context: softbuffer::Context<Arc<Window>>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
}

#[cfg(not(target_os = "macos"))]
impl SoftbufferRenderer {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| format!("softbuffer Context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| format!("softbuffer Surface: {e}"))?;
        Ok(Self {
            _context: context,
            surface,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        use std::num::NonZeroU32;
        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return Ok(());
        };
        self.surface
            .resize(w, h)
            .map_err(|e| format!("softbuffer resize: {e}"))
    }

    pub fn present(&mut self, canvas: &Pixmap, dirty: Option<DirtyRect>) -> Result<(), String> {
        if let Some(rect) = dirty {
            let canvas_w = canvas.width();
            let mut buffer = self
                .surface
                .buffer_mut()
                .map_err(|e| format!("softbuffer buffer_mut: {e}"))?;
            let src = canvas.data();
            let pix_stride = canvas_w as usize;
            for y in rect.y0..rect.y1 {
                let src_row = (y as usize) * pix_stride * 4;
                let dst_row = (y as usize) * pix_stride;
                for x in rect.x0..rect.x1 {
                    let i = x as usize;
                    let c = &src[src_row + i * 4..src_row + i * 4 + 4];
                    buffer[dst_row + i] = u32::from_be_bytes([c[3], c[0], c[1], c[2]]);
                }
            }
            buffer
                .present()
                .map_err(|e| format!("softbuffer present: {e}"))?;
        } else {
            // Nothing to draw this frame; still present to keep softbuffer's
            // frame model alive.
            let buffer = self
                .surface
                .buffer_mut()
                .map_err(|e| format!("softbuffer buffer_mut: {e}"))?;
            buffer
                .present()
                .map_err(|e| format!("softbuffer present: {e}"))?;
        }
        Ok(())
    }
}
