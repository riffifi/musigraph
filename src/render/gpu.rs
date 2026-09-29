use crate::art::CoverArt;
use fontdue::Font;
use std::{collections::HashMap, sync::Arc};
use winit::window::Window;
#[repr(C)]
#[derive(Clone, Copy)]
struct Quad {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}
#[derive(Clone, Copy)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);
impl Color {
    fn array(self) -> [f32; 4] {
        [self.0, self.1, self.2, self.3]
    }
}
struct Glyph {
    uv: [f32; 4],
    w: f32,
    h: f32,
    x: f32,
    y: f32,
    advance: f32,
}
pub struct Gpu {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    dims: wgpu::Buffer,
    dims_bind: wgpu::BindGroup,
    capacity: usize,
    atlas: wgpu::Texture,
    spectrogram: wgpu::Texture,
    cover_texture: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    spec_bind: wgpu::BindGroup,
    cover_bind: wgpu::BindGroup,
    cover_source: Option<CoverArt>,
    font: Font,
    glyphs: HashMap<(char, u32), Glyph>,
    atlas_x: u32,
    atlas_y: u32,
    atlas_row: u32,
    draws: Vec<(u8, Quad)>,
    quads: Vec<Quad>,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}
const SHADER: &str = r#"
struct Out { @builtin(position) pos:vec4f, @location(0) uv:vec2f, @location(1) color:vec4f };
@group(0) @binding(0) var tex:texture_2d<f32>;
@group(0) @binding(1) var samp:sampler;
struct V { @location(0) rect:vec4f,@location(1) uv:vec4f,@location(2) color:vec4f };
@vertex fn vs(@builtin(vertex_index) index:u32,v:V)->Out { var corners=array<vec2f,6>(vec2f(0.,0.),vec2f(1.,0.),vec2f(0.,1.),vec2f(0.,1.),vec2f(1.,0.),vec2f(1.,1.));let c=corners[index];var o:Out;var point=v.rect.xy+c*v.rect.zw;if(v.uv.x<0.){let a=v.rect.xy;let b=v.rect.zw;let d=b-a;let perp=vec2f(-d.y,d.x)/max(length(d),0.00001);point=mix(a,b,c.x)+perp*(c.y-0.5)/dims.z;o.uv=vec2f(0.5/1024.,0.5/1024.);}else{o.uv=v.uv.xy+c*v.uv.zw;}o.pos=vec4f(point*dims.z/dims.xy*vec2f(2.,-2.)+vec2f(-1.,1.),0.,1.);o.color=v.color;return o;}
@fragment fn fs(i:Out)->@location(0) vec4f {let t=textureSample(tex,samp,i.uv);return vec4f(i.color.rgb*t.rgb,i.color.a*t.a);}
"#;
impl Gpu {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or("no GPU adapter")?;
        let (device, queue) = block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        ))
        .map_err(|e| e.to_string())?;
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
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1024,
                height: 1024,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let spectrogram = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 2048,
                height: 512,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let cover_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bind = |tex: &wgpu::Texture| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &tex.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let atlas_bind = bind(&atlas);
        let spec_bind = bind(&spectrogram);
        let cover_bind = bind(&cover_texture);
        let font = Font::from_bytes(
            include_bytes!("../../assets/mono.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .map_err(|e| e.to_string())?;
        let this = Self::create_pipeline(
            window,
            surface,
            device,
            queue,
            config,
            atlas,
            spectrogram,
            atlas_bind,
            spec_bind,
            cover_texture,
            cover_bind,
            font,
            layout,
        )?;
        this.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &this.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255, 255, 255, 255],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        Ok(this)
    }
    fn create_pipeline(
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        atlas: wgpu::Texture,
        spectrogram: wgpu::Texture,
        atlas_bind: wgpu::BindGroup,
        spec_bind: wgpu::BindGroup,
        cover_texture: wgpu::Texture,
        cover_bind: wgpu::BindGroup,
        font: Font,
        layout: wgpu::BindGroupLayout,
    ) -> Result<Self, String> {
        let source = SHADER.replace(
            "struct Out",
            "@group(1) @binding(0) var<uniform> dims:vec4f;\nstruct Out",
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let dims_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout, &dims_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 48,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 32,
                            shader_location: 2,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 48 * 65536,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dims = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dims_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &dims_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dims.as_entire_binding(),
            }],
        });
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            buffer,
            dims,
            dims_bind,
            capacity: 65536,
            atlas,
            spectrogram,
            cover_texture,
            atlas_bind,
            spec_bind,
            cover_bind,
            cover_source: None,
            font,
            glyphs: HashMap::new(),
            atlas_x: 1,
            atlas_y: 1,
            atlas_row: 0,
            draws: Vec::new(),
            quads: Vec::new(),
            width: 0.,
            height: 0.,
            scale: 1.,
        })
    }
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.scale = self.window.scale_factor() as f32;
        self.width = w as f32 / self.scale;
        self.height = h as f32 / self.scale;
        self.surface.configure(&self.device, &self.config);
    }
    pub fn clear(&mut self) {
        self.draws.clear();
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        if w <= 0. || h <= 0. {
            return;
        }
        self.draws.push((
            0,
            Quad {
                rect: [
                    x,
                    y,
                    if w == 1. { 1. / self.scale } else { w },
                    if h == 1. { 1. / self.scale } else { h },
                ],
                uv: [0.5 / 1024., 0.5 / 1024., 0., 0.],
                color: c.array(),
            },
        ));
    }
    pub fn spec(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4]) {
        self.draws.push((
            1,
            Quad {
                rect: [x, y, w, h],
                uv,
                color: [1., 1., 1., 1.],
            },
        ));
    }
    pub fn cover(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4]) {
        self.draws.push((
            2,
            Quad {
                rect: [x, y, w, h],
                uv,
                color: [1., 1., 1., 1.],
            },
        ));
    }
    pub fn set_cover(&mut self, art: Option<&CoverArt>) {
        let Some(art) = art else {
            self.cover_source = None;
            return;
        };
        if self
            .cover_source
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(&cached.encoded, &art.encoded))
        {
            return;
        }
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.cover_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &art.pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(art.width * 4),
                rows_per_image: Some(art.height),
            },
            wgpu::Extent3d {
                width: art.width,
                height: art.height,
                depth_or_array_layers: 1,
            },
        );
        self.cover_source = Some(art.clone());
    }
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color) {
        self.draws.push((
            0,
            Quad {
                rect: [x1, y1, x2, y2],
                uv: [-1., 0., 0., 0.],
                color: c.array(),
            },
        ));
    }
    fn glyph(&mut self, ch: char, size: f32) -> Option<Glyph> {
        if let Some(g) = self.glyphs.get(&(ch, (size * self.scale) as u32)) {
            return Some(Glyph {
                uv: g.uv,
                w: g.w,
                h: g.h,
                x: g.x,
                y: g.y,
                advance: g.advance,
            });
        }
        let (metric, pixels) = self.font.rasterize(ch, size * self.scale);
        let w = metric.width as u32;
        let h = metric.height as u32;
        if self.atlas_x + w + 2 >= 1024 {
            self.atlas_x = 1;
            self.atlas_y += self.atlas_row + 2;
            self.atlas_row = 0;
        }
        if self.atlas_y + h + 2 >= 1024 {
            return None;
        }
        let x = self.atlas_x;
        let y = self.atlas_y;
        self.atlas_x += w + 2;
        self.atlas_row = self.atlas_row.max(h);
        if w > 0 && h > 0 {
            let mut rgba = vec![0; w as usize * h as usize * 4];
            for (i, &a) in pixels.iter().enumerate() {
                rgba[4 * i] = 255;
                rgba[4 * i + 1] = 255;
                rgba[4 * i + 2] = 255;
                rgba[4 * i + 3] = a;
            }
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        let g = Glyph {
            uv: [
                x as f32 / 1024.,
                y as f32 / 1024.,
                w as f32 / 1024.,
                h as f32 / 1024.,
            ],
            w: w as f32 / self.scale,
            h: h as f32 / self.scale,
            x: metric.xmin as f32 / self.scale,
            y: metric.ymin as f32 / self.scale,
            advance: metric.advance_width / self.scale,
        };
        self.glyphs.insert(
            (ch, (size * self.scale) as u32),
            Glyph {
                uv: g.uv,
                w: g.w,
                h: g.h,
                x: g.x,
                y: g.y,
                advance: g.advance,
            },
        );
        Some(g)
    }
    pub fn text(&mut self, x: f32, y: f32, size: f32, s: &str, c: Color) {
        let mut xx = x;
        for ch in s.chars() {
            if let Some(g) = self.glyph(ch, size) {
                if g.w > 0. && g.h > 0. {
                    self.draws.push((
                        0,
                        Quad {
                            rect: [xx + g.x, y - g.y - g.h, g.w, g.h],
                            uv: g.uv,
                            color: c.array(),
                        },
                    ));
                }
                xx += g.advance;
            }
        }
    }
    pub fn upload_spec(&self, pixels: &[u8], w: u32, h: u32) {
        if w > 0 && h > 0 && w <= 2048 && h <= 512 {
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.spectrogram,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    pub fn render(&mut self, bg: Color) {
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let vals = [
            self.config.width as f32,
            self.config.height as f32,
            self.scale,
            0.,
        ];
        self.queue.write_buffer(&self.dims, 0, unsafe {
            std::slice::from_raw_parts(vals.as_ptr() as *const u8, 16)
        });
        if self.draws.len() > self.capacity {
            self.capacity = self.draws.len().next_power_of_two();
            self.buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (self.capacity * 48) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.quads.clear();
        self.quads.extend(self.draws.iter().map(|x| x.1));
        self.queue.write_buffer(&self.buffer, 0, unsafe {
            std::slice::from_raw_parts(self.quads.as_ptr() as *const u8, self.quads.len() * 48)
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.0 as f64,
                            g: bg.1 as f64,
                            b: bg.2 as f64,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.buffer.slice(..));
            pass.set_bind_group(1, &self.dims_bind, &[]);
            let mut i = 0;
            while i < self.draws.len() {
                let kind = self.draws[i].0;
                let mut j = i + 1;
                while j < self.draws.len() && self.draws[j].0 == kind {
                    j += 1;
                }
                pass.set_bind_group(
                    0,
                    if kind == 0 {
                        &self.atlas_bind
                    } else if kind == 1 {
                        &self.spec_bind
                    } else {
                        &self.cover_bind
                    },
                    &[],
                );
                pass.draw(0..6, i as u32..j as u32);
                i = j;
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    let mut f = Box::pin(future);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
