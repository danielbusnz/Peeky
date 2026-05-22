//! macOS-specific window configuration and rendering for the cursor overlay.
//!
//! Handles the platform quirks that make transparent overlays work on macOS:
//! - Manual window sizing (fullscreen mode kills transparency)
//! - NSWindow/CALayer opacity configuration via Objective-C runtime
//! - Retina display scale factor adjustments
//! - wgpu-based pixel presentation (softbuffer strips alpha on macOS)

use std::sync::Arc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tiny_skia::Pixmap;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// Size the window to fill the entire virtual desktop (all monitors).
/// On macOS, `Fullscreen::Borderless` puts the window in its own Space and
/// forces it opaque, killing the transparency we set via `with_transparent(true)`.
/// By spanning all monitors, the cursor overlay follows across displays.
pub fn configure_window_size(event_loop: &ActiveEventLoop, window: &Window) {
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    if monitors.is_empty() {
        return;
    }

    // Calculate the bounding box of all monitors
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for monitor in &monitors {
        let pos = monitor.position();
        let size = monitor.size();
        min_x = min_x.min(pos.x);
        min_y = min_y.min(pos.y);
        max_x = max_x.max(pos.x + size.width as i32);
        max_y = max_y.max(pos.y + size.height as i32);
    }

    let width = (max_x - min_x) as u32;
    let height = (max_y - min_y) as u32;

    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
    window.set_outer_position(winit::dpi::PhysicalPosition::new(min_x, min_y));
}

/// Scale logical cursor coordinates to physical pixels for Retina displays.
/// The `mouse_position` crate returns logical points on macOS but physical
/// pixels on X11, so we scale only on macOS. The canvas is sized in physical
/// pixels (`window.inner_size()` returns physical), so without this the sprite
/// renders in the upper-left quadrant on Retina displays.
pub fn scale_cursor_position(window: &Window, pos: (f64, f64)) -> (f64, f64) {
    let sf = window.scale_factor();
    (pos.0 * sf, pos.1 * sf)
}

/// Configure the NSWindow for transparency.
/// Even with wgpu's PostMultiplied alpha mode, the NSWindow itself needs to
/// be configured as non-opaque with a clear background color.
///
/// # Safety
/// Uses raw Objective-C messaging to configure NSWindow properties.
pub unsafe fn configure_window_transparency(window: &Arc<Window>) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() else {
        return;
    };
    let ns_view = appkit_handle.ns_view.as_ptr() as *mut AnyObject;
    if ns_view.is_null() {
        return;
    }
    let ns_window: *mut AnyObject = msg_send![ns_view, window];
    if ns_window.is_null() {
        return;
    }

    // NSWindow: setOpaque:NO and backgroundColor = [NSColor clearColor]
    let _: () = msg_send![ns_window, setOpaque: Bool::NO];
    let ns_color_class = objc2::class!(NSColor);
    let clear_color: *mut AnyObject = msg_send![ns_color_class, clearColor];
    let _: () = msg_send![ns_window, setBackgroundColor: clear_color];

    // Click-through: mouse events pass through to apps below
    let _: () = msg_send![ns_window, setIgnoresMouseEvents: Bool::YES];

    // Use screenSaver level (1000) - same as Clicky. With proper LSUIElement
    // in Info.plist, this is sufficient for fullscreen overlay.
    const NS_SCREEN_SAVER_WINDOW_LEVEL: i64 = 1000;
    let _: () = msg_send![ns_window, setLevel: NS_SCREEN_SAVER_WINDOW_LEVEL];

    // Prevent the window from hiding when app loses focus.
    let _: () = msg_send![ns_window, setHidesOnDeactivate: Bool::NO];

    // Disable shadow for a cleaner overlay appearance
    let _: () = msg_send![ns_window, setHasShadow: Bool::NO];

    // Collection behavior for fullscreen overlay:
    //   NSWindowCollectionBehaviorCanJoinAllSpaces      (1 << 0 = 1)
    //   NSWindowCollectionBehaviorStationary            (1 << 4 = 16)
    //   NSWindowCollectionBehaviorFullScreenAuxiliary   (1 << 8 = 256)
    //   NSWindowCollectionBehaviorIgnoresCycle          (1 << 6 = 64)
    //
    // IgnoresCycle prevents the window from being cycled to via Cmd+`
    let collection_behavior: u64 = 1 | 16 | 64 | 256;
    let _: () = msg_send![ns_window, setCollectionBehavior: collection_behavior];

    // Prevent window from being released when closed
    let _: () = msg_send![ns_window, setReleasedWhenClosed: Bool::NO];

    // Force the window to front
    let _: () = msg_send![ns_window, orderFrontRegardless];

    eprintln!("[macos] window configured for fullscreen overlay");
}

// ────────────────────────────────────────────────────────────────────────
// WgpuRenderer
// ────────────────────────────────────────────────────────────────────────
//
// Replaces softbuffer on macOS because softbuffer 0.4 hardcodes
// CGImageAlphaInfo::NoneSkipFirst, which strips alpha at the CG level
// before the compositor ever sees it. wgpu's surface configuration
// supports CompositeAlphaMode::PostMultiplied which honors per-pixel alpha.
//
// Each frame we upload the entire tiny-skia Pixmap as a 2D texture and
// render it to the swapchain via a fullscreen-triangle pipeline. The
// fragment shader just samples the texture; no per-pixel work.

const SHADER: &str = r#"
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle: three verts that cover [-1,1] x [-1,1] with
    // UVs that map (0,0) at top-left to (1,1) at bottom-right.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t, s, in.uv);
}
"#;

pub struct WgpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
    sampler: wgpu::Sampler,
}

impl WgpuRenderer {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let w = size.width.max(1);
        let h = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or("no compatible wgpu adapter")?;

        // Use the adapter's limits to support high-resolution displays.
        // downlevel_defaults() caps max_texture_dimension_2d at 2048 which is
        // too small for Retina screens (e.g., 2940x1912).
        let adapter_limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("aegis cursor device"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter_limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        // Pick the first non-srgb format if available; otherwise first format.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or_else(|| caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::PostMultiplied,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let texture = create_texture(&device, w, h);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cursor sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cursor bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = make_bind_group(&device, &bind_group_layout, &view, &sampler);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cursor shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cursor pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cursor pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            bind_group,
            texture,
            sampler,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if width == self.config.width && height == self.config.height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.texture = create_texture(&self.device, width, height);
        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group =
            make_bind_group(&self.device, &self.bind_group_layout, &view, &self.sampler);
    }

    /// Upload the whole canvas as a texture and render it to the swapchain.
    /// We don't try to do dirty-region uploads because wgpu's write_texture
    /// is already a single GPU copy; the canvas is small enough that a full
    /// upload each frame is cheap.
    pub fn present(&mut self, canvas: &Pixmap) -> Result<(), String> {
        let cw = canvas.width();
        let ch = canvas.height();
        if cw == 0 || ch == 0 {
            return Ok(());
        }

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            canvas.data(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(cw * 4),
                rows_per_image: Some(ch),
            },
            wgpu::Extent3d {
                width: cw,
                height: ch,
                depth_or_array_layers: 1,
            },
        );

        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("get_current_texture: {e}"))?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cursor encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cursor pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn create_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cursor texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cursor bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
