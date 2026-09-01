use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.uv = input.uv;
    return output;
}

@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_texture, frame_sampler, input.uv);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

pub struct WgpuPresentation {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    previous_width: u32,
    previous_height: u32,
    last_frame_at: Option<Instant>,
    previous_frame: Option<Vec<u8>>,
}

impl WgpuPresentation {
    pub fn new(window: &sdl2::video::Window) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            dx12_shader_compiler: Default::default(),
            gles_minor_version: wgpu::Gles3MinorVersion::Automatic,
            flags: wgpu::InstanceFlags::default(),
        });
        let target = unsafe {
            wgpu::SurfaceTargetUnsafe::from_window(window)
                .map_err(|error| format!("could not obtain SDL window handles: {error}"))?
        };
        let surface = unsafe {
            instance
                .create_surface_unsafe(target)
                .map_err(|error| format!("could not create WGPU surface: {error}"))?
        };
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            },
        ))
        .ok_or_else(|| "WGPU could not find an adapter for the SDL window".to_string())?;
        let adapter_info = adapter.get_info();
        log!(
            "WGPU presentation adapter: name={:?}, backend={:?}, device={:?}, driver={:?}",
            adapter_info.name,
            adapter_info.backend,
            adapter_info.device,
            adapter_info.driver
        );
        log!("WGPU presentation path is active; frames will be uploaded through the selected WGPU adapter");
        let adapter_limits = adapter.limits();
        log!(
            "WGPU adapter limits: max_storage_buffers_per_shader_stage={}, max_texture_dimension_2d={}, max_bind_groups={}, max_sampled_textures_per_shader_stage={}",
            adapter_limits.max_storage_buffers_per_shader_stage,
            adapter_limits.max_texture_dimension_2d,
            adapter_limits.max_bind_groups,
            adapter_limits.max_sampled_textures_per_shader_stage
        );
        let required_limits = wgpu::Limits::downlevel_defaults()
            .using_resolution(adapter_limits.clone())
            .using_alignment(adapter_limits);
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("RadekHLE WGPU presentation device"),
                required_features: wgpu::Features::empty(),
                required_limits,
            },
            None,
        ))
        .map_err(|error| format!("could not create WGPU device with adapter-compatible limits: {error}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| "WGPU surface reported no supported formats".to_string())?;
        let (width, height) = window.drawable_size();
        let width = width.max(1);
        let height = height.max(1);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RadekHLE WGPU frame presenter"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RadekHLE WGPU frame bindings"),
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
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RadekHLE WGPU frame pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let compilation_options = wgpu::PipelineCompilationOptions {
            constants: &HashMap::new(),
            zero_initialize_workgroup_memory: true,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RadekHLE WGPU frame pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vertex_main",
                compilation_options: compilation_options.clone(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fragment_main",
                compilation_options,
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RadekHLE WGPU frame sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("RadekHLE WGPU frame vertices"),
            contents: bytemuck::cast_slice(&[
                Vertex { position: [-1.0, -1.0], uv: [0.0, 1.0] },
                Vertex { position: [1.0, -1.0], uv: [1.0, 1.0] },
                Vertex { position: [1.0, 1.0], uv: [1.0, 0.0] },
                Vertex { position: [-1.0, -1.0], uv: [0.0, 1.0] },
                Vertex { position: [1.0, 1.0], uv: [1.0, 0.0] },
                Vertex { position: [-1.0, 1.0], uv: [0.0, 0.0] },
            ]),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            vertex_buffer,
            width,
            height,
            previous_width: width,
            previous_height: height,
            last_frame_at: Some(Instant::now()),
            previous_frame: None,
        })
    }

    pub fn present(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        let expected_len = width as usize * height as usize * 4;
        if width == 0 || height == 0 || pixels.len() < expected_len {
            return Err(format!(
                "invalid WGPU frame dimensions {}x{} with {} bytes",
                width,
                height,
                pixels.len()
            ));
        }
        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|error| format!("WGPU surface recovery failed: {error}"))?
            }
            Err(wgpu::SurfaceError::Timeout) => {
                log_dbg!("WGPU presentation skipped a timed-out surface frame");
                return Ok(());
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err("WGPU presentation device ran out of memory".to_string());
            }
        };
        if self.width != output.texture.width() || self.height != output.texture.height() {
            self.width = output.texture.width();
            self.height = output.texture.height();
            self.config.width = self.width.max(1);
            self.config.height = self.height.max(1);
            self.surface.configure(&self.device, &self.config);
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RadekHLE WGPU uploaded frame"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[..expected_len],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RadekHLE WGPU uploaded frame bindings"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let output_view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("RadekHLE WGPU frame encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("RadekHLE WGPU frame pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let output_width = output.texture.width() as f32;
            let output_height = output.texture.height() as f32;
            let source_aspect = width as f32 / height as f32;
            let output_aspect = output_width / output_height;
            let (viewport_width, viewport_height) = if source_aspect > output_aspect {
                (output_width, output_width / source_aspect)
            } else {
                (output_height * source_aspect, output_height)
            };
            pass.set_viewport(
                (output_width - viewport_width) / 2.0,
                (output_height - viewport_height) / 2.0,
                viewport_width,
                viewport_height,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        Ok(())
    }

    pub fn present_pixels(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        bottom_up: bool,
    ) -> Result<(), String> {
        let expected_len = width as usize * height as usize * 4;
        if pixels.len() < expected_len {
            return Err(format!("invalid WGPU frame dimensions {}x{} with {} bytes", width, height, pixels.len()));
        }
        if bottom_up {
            let mut oriented = vec![0u8; expected_len];
            let row_bytes = width as usize * 4;
            for y in 0..height as usize {
                let source = (height as usize - y - 1) * row_bytes;
                let destination = y * row_bytes;
                oriented[destination..destination + row_bytes]
                    .copy_from_slice(&pixels[source..source + row_bytes]);
            }
            self.present(&oriented, width, height)
        } else {
            self.present(pixels, width, height)
        }
    }

    pub fn present_interpolated(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        bottom_up: bool,
        refresh_rate: f64,
    ) -> Result<(), String> {
        let expected_len = width as usize * height as usize * 4;
        if width == 0 || height == 0 || pixels.len() < expected_len {
            return Err(format!("invalid WGPU frame dimensions {}x{} with {} bytes", width, height, pixels.len()));
        }
        let mut current = vec![0u8; expected_len];
        let row_bytes = width as usize * 4;
        for y in 0..height as usize {
            let source_y = if bottom_up { height as usize - y - 1 } else { y };
            let source = source_y * row_bytes;
            let destination = y * row_bytes;
            current[destination..destination + row_bytes]
                .copy_from_slice(&pixels[source..source + row_bytes]);
        }
        let interval = Duration::from_secs_f64(1.0 / refresh_rate.max(1.0));
        if let Some(previous) = self.previous_frame.take() {
            if self.previous_width == width && self.previous_height == height {
                let elapsed = self.last_frame_at.map_or(interval, |last| last.elapsed());
                let slots = if elapsed <= Duration::from_millis(250) {
                    (elapsed.as_secs_f64() / interval.as_secs_f64())
                        .round()
                        .clamp(1.0, 8.0) as usize
                } else {
                    1
                };
                for step in 1..slots {
                    let blend = step as u32 * 256 / slots as u32;
                    let inverse = 256 - blend;
                    let mut generated = vec![0u8; expected_len];
                    for index in 0..expected_len {
                        generated[index] = ((previous[index] as u32 * inverse
                            + current[index] as u32 * blend) >> 8) as u8;
                    }
                    self.present(&generated, width, height)?;
                }
            }
        }
        self.present(&current, width, height)?;
        self.previous_width = width;
        self.previous_height = height;
        self.last_frame_at = Some(Instant::now());
        self.previous_frame = Some(current);
        Ok(())
    }
}

pub fn describe() -> &'static str {
    "WGPU host presentation backend with the existing GLES guest compatibility path"
}
