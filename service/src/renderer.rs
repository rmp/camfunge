//! Headless GPU pipeline (wgpu: Metal / Vulkan / DX12 / GL). Same passes as the original WebGL app:
//! camera upload → motion pass → 64-frame history atlas → filter (with feedback) → BGRA output, read back
//! to the CPU for the virtual camera and preview.

use anyhow::{anyhow, Context, Result};
use std::borrow::Cow;

use crate::engine::{IngestPlan, Uniforms};
use crate::filters::{COPY, FILTERS, MOTION};
use crate::shaders::{self, binding, HISTORY_FRAMES, TILE_H, TILE_W};

/// Motion is tracked at a quarter of the output resolution.
pub const MOTION_DOWNSCALE: u32 = 4;
const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const CAMERA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// BGRA is what the macOS virtual camera wants; the other sinks convert from it.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn target(device: &wgpu::Device, label: &str, [w, h]: [u32; 2], format: wgpu::TextureFormat, extra: wgpu::TextureUsages) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | extra,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Target { texture, view }
}

struct Camera {
    size: [u32; 2],
    /// [current, previous]; swapped on every ingest.
    frames: [Target; 2],
    bg: Target,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub gpu: String,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    filters: Vec<wgpu::RenderPipeline>,
    motion_pass: wgpu::RenderPipeline,
    copy: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    size: [u32; 2],
    camera: Option<Camera>,
    blank: Target,
    feedback: [Target; 2],
    motion: [Target; 2],
    history: Target,
    output: Target,
    readback: wgpu::Buffer,
    padded_row: u32,
    read: usize,
    motion_read: usize,
    frame_out: Vec<u8>,
}

impl Renderer {
    /// Opens the best available GPU, falling back to a software adapter (e.g. lavapipe, WARP) if there is none.
    pub fn new(size: [u32; 2]) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: true, ..Default::default() }))
        })
        .context("no GPU adapter available")?;
        let info = adapter.get_info();
        let gpu = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("camfunge"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("could not open GPU device")?;
        Self::with_device(device, queue, gpu, size)
    }

    fn with_device(device: wgpu::Device, queue: wgpu::Queue, gpu: String, size: [u32; 2]) -> Result<Self> {
        let max = device.limits().max_texture_dimension_2d;
        if size[0] == 0 || size[1] == 0 || size[0] > max || size[1] > max || size[0] % 2 == 1 || size[1] % 2 == 1 {
            return Err(anyhow!("output size {}x{} must be even and within 2..={max}", size[0], size[1]));
        }
        let layout_entries: Vec<_> = [wgpu::BindGroupLayoutEntry {
            binding: binding::UNIFORMS,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(shaders::UNIFORM_BYTES),
            },
            count: None,
        }, wgpu::BindGroupLayoutEntry {
            binding: binding::SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }]
        .into_iter()
        .chain(binding::TEXTURES.map(|b| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }))
        .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("passes"), entries: &layout_entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("passes"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(shaders::VERTEX)),
        });

        // Shader compile errors surface asynchronously; capture them so a bad filter names itself.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = |name: &str, body: &str, format: wgpu::TextureFormat| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Glsl { shader: Cow::Owned(shaders::fragment(body)), stage: wgpu::naga::ShaderStage::Fragment, defines: &[] },
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &vertex, entry_point: Some("main"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let filters = FILTERS.iter().map(|f| pipeline(f.name, f.src, FEEDBACK_FORMAT)).collect();
        let motion_pass = pipeline("motion", MOTION, FEEDBACK_FORMAT);
        let copy = pipeline("history copy", COPY, CAMERA_FORMAT);
        let blit = pipeline("output blit", shaders::BLIT, OUTPUT_FORMAT);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(anyhow!("shader pipeline failed: {e}"));
        }

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mirrored linear"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: shaders::UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let motion_size = [size[0].div_ceil(MOTION_DOWNSCALE), size[1].div_ceil(MOTION_DOWNSCALE)];
        let padded_row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: padded_row as u64 * size[1] as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Renderer {
            blank: target(&device, "blank", [1, 1], CAMERA_FORMAT, wgpu::TextureUsages::empty()),
            feedback: [0, 1].map(|_| target(&device, "feedback", size, FEEDBACK_FORMAT, rt)),
            motion: [0, 1].map(|_| target(&device, "motion", motion_size, FEEDBACK_FORMAT, rt | wgpu::TextureUsages::COPY_SRC)),
            history: target(&device, "history", [TILE_W * 8, TILE_H * 8], CAMERA_FORMAT, rt),
            output: target(&device, "output", size, OUTPUT_FORMAT, rt | wgpu::TextureUsages::COPY_SRC),
            device,
            queue,
            gpu,
            layout,
            sampler,
            uniforms,
            filters,
            motion_pass,
            copy,
            blit,
            size,
            camera: None,
            readback,
            padded_row,
            read: 0,
            motion_read: 0,
            frame_out: vec![0; (size[0] * size[1] * 4) as usize],
        })
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    fn bind_group(&self, textures: [&wgpu::TextureView; 6]) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: binding::UNIFORMS, resource: self.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: binding::SAMPLER, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ];
        for (b, view) in binding::TEXTURES.into_iter().zip(textures) {
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(view) });
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.layout, entries: &entries })
    }

    fn draw(
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
        pipeline: &wgpu::RenderPipeline,
        bind_group: &wgpu::BindGroup,
        viewports: &[[f32; 4]],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        if viewports.is_empty() {
            pass.draw(0..3, 0..1);
        }
        for &[x, y, w, h] in viewports {
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }

    /// Uploads `rgba` (tightly packed, `size` pixels) as the newest camera frame.
    fn upload(&mut self, rgba: &[u8], size: [u32; 2]) {
        if self.camera.as_ref().is_none_or(|c| c.size != size) {
            let usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
            self.camera = Some(Camera {
                size,
                frames: [0, 1].map(|_| target(&self.device, "camera", size, CAMERA_FORMAT, usage)),
                bg: target(&self.device, "background", size, CAMERA_FORMAT, wgpu::TextureUsages::COPY_DST),
            });
        }
        let cam = self.camera.as_mut().expect("just created");
        cam.frames.swap(0, 1);
        self.queue.write_texture(
            cam.frames[0].texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: None },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
    }

    /// Renders one output frame and returns it as tightly packed BGRA, top row first.
    ///
    /// `camera` is a fresh camera frame (RGBA, its size, and what [`crate::engine::Engine::ingest`] planned),
    /// or `None` to re-render over the last one.
    pub fn render(&mut self, camera: Option<(&[u8], [u32; 2], IngestPlan)>, filter: usize, uniforms: &Uniforms) -> Result<&[u8]> {
        self.queue.write_buffer(&self.uniforms, 0, &uniforms.to_bytes());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);

        if let Some((rgba, size, plan)) = camera {
            self.upload(rgba, size);
            let cam = self.camera.as_ref().expect("uploaded");
            if plan.grab_background {
                encoder.copy_texture_to_texture(cam.frames[0].texture.as_image_copy(), cam.bg.texture.as_image_copy(), cam.frames[0].texture.size());
            }
            let (now, prev) = (&cam.frames[0].view, &cam.frames[1].view);
            let blank = &self.blank.view;

            let bind = self.bind_group([now, &self.motion[self.motion_read].view, prev, blank, blank, blank]);
            Self::draw(&mut encoder, &self.motion[1 - self.motion_read].view, clear, &self.motion_pass, &bind, &[]);
            self.motion_read = 1 - self.motion_read;

            let tiles: Vec<u32> = if plan.fill_history { (0..HISTORY_FRAMES).collect() } else { vec![plan.head] };
            // Tile t is at column t%8, row t/8 counting up from the bottom (v up), i.e. 7-row from the top.
            let viewports: Vec<[f32; 4]> = tiles
                .iter()
                .map(|t| [((t % 8) * TILE_W) as f32, ((7 - t / 8) * TILE_H) as f32, TILE_W as f32, TILE_H as f32])
                .collect();
            let bind = self.bind_group([now, blank, prev, blank, blank, blank]);
            Self::draw(&mut encoder, &self.history.view, wgpu::LoadOp::Load, &self.copy, &bind, &viewports);
        }

        let blank = &self.blank.view;
        let (cam, bg) = self.camera.as_ref().map_or((blank, blank), |c| (&c.frames[0].view, &c.bg.view));
        let prev_cam = self.camera.as_ref().map_or(blank, |c| &c.frames[1].view);
        let bind = self.bind_group([cam, &self.feedback[self.read].view, prev_cam, &self.motion[self.motion_read].view, bg, &self.history.view]);
        let filter = &self.filters[filter % self.filters.len()];
        Self::draw(&mut encoder, &self.feedback[1 - self.read].view, clear, filter, &bind, &[]);
        self.read = 1 - self.read;

        let bind = self.bind_group([blank, &self.feedback[self.read].view, blank, blank, blank, blank]);
        Self::draw(&mut encoder, &self.output.view, clear, &self.blit, &bind, &[]);

        encoder.copy_texture_to_buffer(
            self.output.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: None },
            },
            self.output.texture.size(),
        );
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.queue.submit([encoder.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(anyhow!("GPU validation error: {e}"));
        }

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("GPU poll failed")?;
        rx.recv().context("GPU map callback dropped")?.context("GPU readback failed")?;
        {
            let mapped = slice.get_mapped_range().map_err(|e| anyhow!("readback mapping: {e:?}"))?;
            let row = (self.size[0] * 4) as usize;
            for (y, out) in self.frame_out.chunks_exact_mut(row).enumerate() {
                let start = y * self.padded_row as usize;
                out.copy_from_slice(&mapped[start..start + row]);
            }
        }
        self.readback.unmap();
        Ok(&self.frame_out)
    }

    /// Reads back the motion texture's red channel (recent-motion heat); for tests and diagnostics.
    pub fn motion_heat_max(&self) -> Result<f32> {
        let tex = &self.motion[self.motion_read].texture;
        let size = tex.size();
        let row = (size.width * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("motion readback"),
            size: row as u64 * size.height as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            size,
        );
        self.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("GPU poll failed")?;
        let mapped = slice.get_mapped_range().map_err(|e| anyhow!("{e:?}"))?;
        let mut max = 0.0f32;
        for y in 0..size.height as usize {
            for x in 0..size.width as usize {
                let p = y * row as usize + x * 8;
                max = max.max(half_to_f32(u16::from_le_bytes([mapped[p], mapped[p + 1]])));
            }
        }
        Ok(max)
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::half_to_f32;

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0), 0.0);
    }
}
