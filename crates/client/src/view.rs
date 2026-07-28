//! winit window + wgpu renderer for the decoded H.264 stream. Runs on the main
//! thread (winit requirement); networking/decoding happens on a background
//! thread and hands frames to this one over a channel — see main.rs.

use crate::decode::DecodedFrame;
use crate::keyboard_input::KeyboardPad;
use anyhow::Result;
use glyphon::{
    Attrs, Buffer as TextBuffer, Color as TextColor, Family, FontSystem, Metrics, Resolution,
    Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
};
use std::sync::{mpsc::Receiver, Arc, Mutex};
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// couchlink brand palette (matches web/src/App.css) — dark background, mint accent.
const BG_COLOR: wgpu::Color = wgpu::Color {
    r: 0.0196,
    g: 0.0314,
    b: 0.0471,
    a: 1.0,
};
const ACCENT_COLOR: TextColor = TextColor::rgb(0x3e, 0xcf, 0x8e);
const MUTED_COLOR: TextColor = TextColor::rgb(0x8f, 0xa0, 0xb5);

struct FrameTextures {
    _y_texture: wgpu::Texture,
    _u_texture: wgpu::Texture,
    _v_texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    current_frame: Option<FrameTextures>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    status_text: TextBuffer,
}

impl Renderer {
    fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = request_adapter(&instance, &surface)?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("couchlink-client"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        ))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(
                r#"
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0),
    );
    var out: VOut;
    out.pos = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;
@group(0) @binding(1) var t_u: texture_2d<f32>;
@group(0) @binding(2) var t_v: texture_2d<f32>;
@group(0) @binding(3) var s_frame: sampler;

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let y = textureSample(t_frame, s_frame, in.uv).r;
    let u = textureSample(t_u, s_frame, in.uv).r - 0.5;
    let v = textureSample(t_v, s_frame, in.uv).r - 0.5;
    let rgb = vec3<f32>(
        y + 1.5748 * v,
        y - 0.1873 * u - 0.4681 * v,
        y + 1.8556 * u
    );
    return vec4<f32>(rgb, 1.0);
}
"#,
            )),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frame-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(config.format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let mut text_atlas = TextAtlas::new(&device, &queue, config.format);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);

        let mut status_text = TextBuffer::new(&mut font_system, Metrics::new(22.0, 30.0));
        status_text.set_size(&mut font_system, config.width as f32, config.height as f32);
        let title_attrs = Attrs::new().family(Family::SansSerif).color(ACCENT_COLOR);
        let subtitle_attrs = Attrs::new().family(Family::SansSerif).color(MUTED_COLOR);
        status_text.set_rich_text(
            &mut font_system,
            [("couchlink\n", title_attrs), ("Waiting for stream…", subtitle_attrs)],
            Shaping::Advanced,
        );
        status_text.shape_until_scroll(&mut font_system);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            current_frame: None,
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            status_text,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.status_text
            .set_size(&mut self.font_system, width as f32, height as f32);
    }

    fn upload_frame(&mut self, frame: &DecodedFrame) {
        if self
            .current_frame
            .as_ref()
            .map(|textures| textures.width != frame.width || textures.height != frame.height)
            .unwrap_or(true)
        {
            self.current_frame = Some(self.create_frame_textures(frame.width, frame.height));
        }

        let textures = self.current_frame.as_ref().expect("frame textures initialized");
        upload_plane(&self.queue, &textures._y_texture, frame.width, frame.height, &frame.y_plane);
        upload_plane(
            &self.queue,
            &textures._u_texture,
            frame.width / 2,
            frame.height / 2,
            &frame.u_plane,
        );
        upload_plane(
            &self.queue,
            &textures._v_texture,
            frame.width / 2,
            frame.height / 2,
            &frame.v_plane,
        );
    }

    fn draw(&mut self) -> Result<()> {
        let show_status = self.current_frame.is_none();
        if show_status {
            self.text_renderer
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.font_system,
                    &mut self.text_atlas,
                    Resolution {
                        width: self.config.width,
                        height: self.config.height,
                    },
                    [
                        TextArea {
                            buffer: &self.status_text,
                            left: (self.config.width as f32 / 2.0 - 90.0).max(16.0),
                            top: (self.config.height as f32 / 2.0 - 40.0).max(16.0),
                            scale: 1.0,
                            bounds: TextBounds::default(),
                            default_color: ACCENT_COLOR,
                        },
                    ],
                    &mut self.swash_cache,
                )
                .map_err(|e| anyhow::anyhow!("text prepare: {e:?}"))?;
        }

        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BG_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(frame) = &self.current_frame {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &frame.bind_group, &[]);
                pass.draw(0..6, 0..1);
            } else {
                self.text_renderer
                    .render(&self.text_atlas, &mut pass)
                    .map_err(|e| anyhow::anyhow!("text render: {e:?}"))?;
            }
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        self.text_atlas.trim();
        Ok(())
    }

    fn create_frame_textures(&self, width: u32, height: u32) -> FrameTextures {
        let y_texture = create_plane_texture(&self.device, width, height, "frame-y-texture");
        let u_texture = create_plane_texture(&self.device, width / 2, height / 2, "frame-u-texture");
        let v_texture = create_plane_texture(&self.device, width / 2, height / 2, "frame-v-texture");
        let y_view = y_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let u_view = u_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let v_view = v_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&u_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&v_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        FrameTextures {
            _y_texture: y_texture,
            _u_texture: u_texture,
            _v_texture: v_texture,
            bind_group,
            width,
            height,
        }
    }
}

fn request_adapter(instance: &wgpu::Instance, surface: &wgpu::Surface<'static>) -> Result<wgpu::Adapter> {
    let attempts = [
        ("high-performance", wgpu::PowerPreference::HighPerformance, false),
        ("default", wgpu::PowerPreference::default(), false),
        ("fallback", wgpu::PowerPreference::LowPower, true),
    ];
    for (label, power_preference, force_fallback_adapter) in attempts {
        if let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            compatible_surface: Some(surface),
            force_fallback_adapter,
        })) {
            let info = adapter.get_info();
            info!(
                "using {label} adapter: {} ({:?}/{:?})",
                info.name, info.backend, info.device_type
            );
            return Ok(adapter);
        }
    }
    Err(anyhow::anyhow!("no compatible GPU adapter"))
}

fn create_plane_texture(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload_plane(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32, data: &[u8]) {
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    frame_rx: Receiver<DecodedFrame>,
    keyboard_pad: Arc<Mutex<KeyboardPad>>,
    shutdown_tx: std::sync::mpsc::Sender<()>,
    init_error: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("couchlink");
        match event_loop.create_window(attrs) {
            Ok(w) => {
                let window = Arc::new(w);
                match Renderer::new(window.clone()) {
                    Ok(r) => {
                        self.renderer = Some(r);
                        self.window = Some(window);
                    }
                    Err(e) => {
                        self.init_error = Some(e);
                        event_loop.exit();
                    }
                }
            }
            Err(e) => {
                self.init_error = Some(anyhow::anyhow!("window creation failed: {e}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let _ = self.shutdown_tx.send(());
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent {
                    physical_key: PhysicalKey::Code(code),
                    state,
                    logical_key,
                    ..
                },
                ..
            } => {
                if state == ElementState::Pressed && logical_key == Key::Named(NamedKey::Escape) {
                    let _ = self.shutdown_tx.send(());
                    event_loop.exit();
                    return;
                }
                if state == ElementState::Pressed
                    && (logical_key == Key::Named(NamedKey::F11) || code == KeyCode::KeyF)
                {
                    if let Some(w) = &self.window {
                        let fullscreen = w.fullscreen().is_some();
                        w.set_fullscreen(if fullscreen {
                            None
                        } else {
                            Some(winit::window::Fullscreen::Borderless(None))
                        });
                    }
                    return;
                }
                let mut kp = self.keyboard_pad.lock().unwrap();
                kp.set_key(code, state == ElementState::Pressed);
            }
            WindowEvent::RedrawRequested => {
                while let Ok(frame) = self.frame_rx.try_recv() {
                    if let Some(r) = &mut self.renderer {
                        r.upload_frame(&frame);
                    }
                }
                if let Some(r) = &mut self.renderer {
                    if let Err(e) = r.draw() {
                        warn!("draw error: {e}");
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Blocks the calling thread (must be the process main thread) running the
/// window until closed, Esc pressed, or window/GPU init fails.
pub fn run(
    frame_rx: Receiver<DecodedFrame>,
    keyboard_pad: Arc<Mutex<KeyboardPad>>,
    shutdown_tx: std::sync::mpsc::Sender<()>,
) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App {
        window: None,
        renderer: None,
        frame_rx,
        keyboard_pad,
        shutdown_tx,
        init_error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.init_error {
        return Err(e);
    }
    Ok(())
}
