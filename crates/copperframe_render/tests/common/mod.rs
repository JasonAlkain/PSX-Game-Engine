//! Shared offscreen rendering harness for GPU image tests.
//!
//! Needs a graphics adapter (a software one such as Mesa lavapipe is fine).
//! Without one, tests print `SKIPPED` and return, unless `COPPERFRAME_REQUIRE_GPU=1`
//! is set, in which case a missing adapter fails the test. A skipped run is
//! not a passed render check.

#![allow(dead_code)]

use copperframe_core::RetroStyle;
use copperframe_render::{
    Camera, CompositeSlot, DrawItem, GpuContext, GpuError, GpuMesh, GpuTexture, Lighting, Material,
    MeshData, Rect, RetroRenderer, TextureData, wgpu,
};

pub fn gpu_or_skip() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::headless()) {
        Ok(gpu) => Some(gpu),
        // Only a missing adapter is a skip. An adapter that exists but fails
        // to create a device is a renderer setup failure.
        Err(err @ GpuError::NoAdapter(_)) => {
            if std::env::var("COPPERFRAME_REQUIRE_GPU").as_deref() == Ok("1") {
                panic!("COPPERFRAME_REQUIRE_GPU=1 but no adapter is available: {err}");
            }
            eprintln!("SKIPPED: no graphics adapter available ({err})");
            None
        }
        Err(err) => panic!("GPU initialisation failed: {err}"),
    }
}

/// An RGBA8 image read back from the GPU, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "({x},{y}) out of bounds");
        let i = ((y * self.width + x) * 4) as usize;
        self.rgba[i..i + 4].try_into().unwrap()
    }

    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let p = self.px(x, y);
        [p[0], p[1], p[2]]
    }

    /// Writes a binary PPM under the test target directory (best effort).
    pub fn save_ppm(&self, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for px in self.rgba.chunks(4) {
            out.extend_from_slice(&px[..3]);
        }
        let path = dir.join(name);
        if std::fs::write(&path, out).is_ok() {
            eprintln!("wrote {}", path.display());
        }
    }
}

/// A mesh + texture + material + transform, owned, for building test scenes.
pub struct Object {
    pub mesh: GpuMesh,
    pub texture: GpuTexture,
    pub material: Material,
    pub model: glam::Mat4,
    /// Joint palette for skinned meshes.
    pub joints: Option<Vec<glam::Mat4>>,
}

pub struct Harness {
    pub gpu: GpuContext,
    pub renderer: RetroRenderer,
    pub output_format: wgpu::TextureFormat,
}

impl Harness {
    /// `output_format` is the composite target format. `Rgba8Unorm` reads back
    /// the sRGB-encoded values the composite produced directly.
    pub fn new(gpu: GpuContext, output_format: wgpu::TextureFormat) -> Self {
        let renderer = RetroRenderer::new(&gpu.device, output_format);
        Self {
            gpu,
            renderer,
            output_format,
        }
    }

    pub fn object(
        &self,
        mesh: &MeshData,
        texture: &TextureData,
        material: Material,
        model: glam::Mat4,
    ) -> Object {
        Object {
            mesh: self
                .renderer
                .upload_mesh(&self.gpu.device, mesh)
                .expect("mesh uploads"),
            texture: self
                .renderer
                .upload_texture(&self.gpu.device, &self.gpu.queue, texture)
                .expect("texture uploads"),
            material,
            model,
            joints: None,
        }
    }

    /// Renders `objects` with `style` at `internal` size and composites into
    /// an `output`-sized image at `rect` (or the whole output if `None`).
    pub fn render(
        &self,
        style: &RetroStyle,
        camera: &Camera,
        objects: &[Object],
        internal: (u32, u32),
        output: (u32, u32),
        rect: Option<Rect>,
    ) -> Image {
        self.render_lit(
            style,
            camera,
            objects,
            &Lighting::default(),
            internal,
            output,
            rect,
        )
    }

    /// [`Harness::render`] with explicit lighting.
    #[allow(clippy::too_many_arguments)]
    pub fn render_lit(
        &self,
        style: &RetroStyle,
        camera: &Camera,
        objects: &[Object],
        lighting: &Lighting,
        internal: (u32, u32),
        output: (u32, u32),
        rect: Option<Rect>,
    ) -> Image {
        let device = &self.gpu.device;
        let queue = &self.gpu.queue;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let mut view = self
            .renderer
            .create_view(device, internal.0, internal.1)
            .expect("view");
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test output"),
            size: wgpu::Extent3d {
                width: output.0,
                height: output.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.output_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());

        let items: Vec<DrawItem<'_>> = objects
            .iter()
            .map(|o| DrawItem {
                mesh: &o.mesh,
                texture: &o.texture,
                material: &o.material,
                model: o.model,
                joints: o.joints.as_deref(),
            })
            .collect();
        let rect = rect.unwrap_or(Rect {
            x: 0,
            y: 0,
            width: output.0,
            height: output.1,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        self.renderer.render_view(
            device,
            queue,
            &mut encoder,
            &mut view,
            camera,
            lighting,
            style,
            &items,
        );
        self.renderer.composite(
            queue,
            &mut encoder,
            &target_view,
            output,
            &[CompositeSlot {
                view: &view,
                rect,
                scissor: rect,
                style,
            }],
        );

        let bytes_per_row = (output.0 * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(bytes_per_row * output.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(output.1),
                },
            },
            wgpu::Extent3d {
                width: output.0,
                height: output.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |r| {
            r.expect("readback buffer maps");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
        if let Some(err) = pollster::block_on(scope.pop()) {
            panic!("wgpu validation error during offscreen render: {err}");
        }

        let mapped = readback.slice(..).get_mapped_range().expect("mapped range");
        let mut rgba = Vec::with_capacity((output.0 * output.1 * 4) as usize);
        for row in 0..output.1 {
            let start = (row * bytes_per_row) as usize;
            rgba.extend_from_slice(&mapped[start..start + (output.0 * 4) as usize]);
        }
        Image {
            width: output.0,
            height: output.1,
            rgba,
        }
    }
}

/// A style with every effect off, black background, fog off, so a test can
/// enable exactly the effect it checks.
pub fn bare_style(internal: (u32, u32)) -> RetroStyle {
    let mut s = RetroStyle::clean();
    s.low_res.enabled = true;
    s.low_res.width = internal.0;
    s.low_res.height = internal.1;
    s.fog.enabled = false;
    s.atmosphere.background = [0, 0, 0];
    s
}

pub fn white_texture() -> TextureData {
    TextureData::new(1, 1, vec![255; 4]).unwrap()
}

pub fn unlit() -> Material {
    Material {
        lit: false,
        ..Material::default()
    }
}
