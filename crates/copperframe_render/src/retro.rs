//! The retro renderer: a scene pass into an internal-resolution target, then
//! a composite pass that upscales it into the output with colour reduction
//! and dithering.
//!
//! Stages per frame (see `docs/ARCHITECTURE.md`):
//! 1. [`RetroRenderer::render_view`]: opaque and cutout geometry, then blended
//!    geometry sorted back-to-front, into a [`RenderView`]'s `Rgba16Float`
//!    colour + `Depth32Float` targets. Vertex snapping, affine UVs, lighting
//!    and fog happen here.
//! 2. [`RetroRenderer::composite`]: one or more views mapped into rectangles of
//!    the output (window) with nearest-neighbour upscale, letterbox bars,
//!    sRGB encoding, ordered dithering and colour quantisation.
//!
//! All pipelines, layouts and samplers are created in [`RetroRenderer::new`].
//! Per frame there are uniform writes and two render passes per view; GPU
//! targets are recreated only when a view's size changes.

use bytemuck::{Pod, Zeroable};
use copperframe_core::RetroStyle;
use copperframe_core::style::{MAX_BLOB_SHADOWS, MAX_LOCAL_LIGHTS, TextureFilter, srgb8_to_linear};
use glam::{Mat4, Vec3};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::layout::Rect;
use crate::{Camera, MAX_JOINTS, Material, MeshData, MeshError, TextureData, TextureError};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Linear, high-precision scene colour so quantisation happens once, later.
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const SCENE_SHADER: &str = include_str!("shaders/scene.wgsl");
pub const COMPOSITE_SHADER: &str = include_str!("shaders/composite.wgsl");

/// Per-view uniforms. Must match `FrameUniforms` in `shaders/scene.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog_range: [f32; 4],
    pub snap: [f32; 4],
    pub options: [f32; 4],
    pub shadow_params: [f32; 4],
    pub lights: [[f32; 4]; 2 * MAX_LOCAL_LIGHTS],
    pub blobs: [[f32; 4]; MAX_BLOB_SHADOWS],
}

/// Per-draw uniforms. Must match `DrawUniforms` in `shaders/scene.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DrawUniforms {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub tint: [f32; 4],
    pub params: [f32; 4],
    pub emissive: [f32; 4],
    pub skin: [f32; 4],
    pub uv_tile: [f32; 4],
}

/// One skinned draw's joints. Must match `JointPalette` in
/// `shaders/scene.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct JointPalette {
    pub joints: [[[f32; 4]; 4]; MAX_JOINTS],
}

/// Per-slot composite uniforms. Must match `CompositeUniforms` in
/// `shaders/composite.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct CompositeUniforms {
    pub rect: [f32; 4],
    pub source_size: [f32; 4],
    pub color: [f32; 4],
}

/// Lighting for one view, all colours linear: ambient, one directional
/// light, local point lights and blob shadow casters.
#[derive(Debug, Clone, PartialEq)]
pub struct Lighting {
    /// Direction the light travels *from* the light towards the scene.
    pub direction: Vec3,
    pub color: Vec3,
    pub ambient: Vec3,
    /// Point lights. At most [`MAX_LOCAL_LIGHTS`] are used per view (the
    /// nearest to the camera); none when the style turns local lights off.
    pub points: Vec<PointLight>,
    /// Blob shadow casters. At most [`MAX_BLOB_SHADOWS`] are used (the
    /// nearest to the camera); none when the style turns shadows off.
    pub blobs: Vec<BlobShadow>,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: Vec3::splat(0.85),
            ambient: Vec3::splat(0.25),
            points: Vec::new(),
            blobs: Vec::new(),
        }
    }
}

/// A light with a bounded range: full strength at its position, fading to
/// exactly zero at `range` (`(1 - (d/range)²)²`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub position: Vec3,
    /// Linear colour multiplied by intensity.
    pub color: Vec3,
    pub range: f32,
}

/// A soft dark disc on upward-facing surfaces below a caster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlobShadow {
    /// Centre of the caster's footprint at its lowest point.
    pub bottom: Vec3,
    pub radius: f32,
}

/// The `limit` entries nearest to `eye` among the finite ones.
fn nearest<T: Copy>(
    items: &[T],
    eye: Vec3,
    limit: usize,
    pos: impl Fn(&T) -> (Vec3, f32),
) -> Vec<T> {
    let mut v: Vec<(f32, T)> = items
        .iter()
        .filter_map(|t| {
            let (p, size) = pos(t);
            (p.is_finite() && size.is_finite() && size > 0.0)
                .then(|| ((p.distance(eye) - size).max(0.0), *t))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().take(limit).map(|(_, t)| t).collect()
}

impl FrameUniforms {
    /// Builds the view uniforms. The camera's near/far planes are replaced by
    /// the style's atmosphere settings; `target_size` is the view's colour
    /// target size (the snapping grid).
    pub fn new(
        camera: &Camera,
        lighting: &Lighting,
        style: &RetroStyle,
        target_size: (u32, u32),
    ) -> Self {
        let camera = Camera {
            near: style.atmosphere.near,
            far: style.atmosphere.far,
            ..*camera
        };
        let (w, h) = (target_size.0.max(1) as f32, target_size.1.max(1) as f32);
        let to_light = (-lighting.direction).normalize_or(Vec3::Y);
        let fog = &style.fog;
        let fog_rgb = Vec3::from(srgb8_to_linear(fog.color));
        let snap = &style.vertex_snap;
        Self {
            view_proj: camera.view_projection(w / h).to_cols_array_2d(),
            camera_pos: camera.position.extend(1.0).to_array(),
            light_dir: to_light.extend(0.0).to_array(),
            light_color: lighting.color.extend(1.0).to_array(),
            ambient: lighting.ambient.extend(1.0).to_array(),
            fog_color: fog_rgb
                .extend(if fog.enabled { 1.0 } else { 0.0 })
                .to_array(),
            fog_range: [fog.start, 1.0 / (fog.end - fog.start).max(1.0e-3), 0.0, 0.0],
            snap: [
                w,
                h,
                snap.grid.max(0.01),
                if snap.enabled {
                    snap.strength.clamp(0.0, 1.0)
                } else {
                    0.0
                },
            ],
            options: [0.0; 4],
            shadow_params: [
                style.shadows.opacity.clamp(0.0, 1.0),
                affine_correction(style),
                0.0,
                0.0,
            ],
            lights: [[0.0; 4]; 2 * MAX_LOCAL_LIGHTS],
            blobs: [[0.0; 4]; MAX_BLOB_SHADOWS],
        }
        .with_locals(camera.position, lighting, style)
    }

    fn with_locals(mut self, eye: Vec3, lighting: &Lighting, style: &RetroStyle) -> Self {
        let points = if style.shading.local_lights {
            nearest(&lighting.points, eye, MAX_LOCAL_LIGHTS, |l| {
                (l.position, l.range)
            })
        } else {
            Vec::new()
        };
        for (i, l) in points.iter().enumerate() {
            self.lights[i * 2] = l.position.extend(l.range).to_array();
            self.lights[i * 2 + 1] = l.color.max(Vec3::ZERO).extend(0.0).to_array();
        }
        let blobs = if style.shadows.enabled {
            nearest(&lighting.blobs, eye, MAX_BLOB_SHADOWS, |b| {
                (b.bottom, b.radius)
            })
        } else {
            Vec::new()
        };
        for (i, b) in blobs.iter().enumerate() {
            self.blobs[i] = b.bottom.extend(b.radius).to_array();
        }
        self.options = [
            if style.affine_uv.enabled { 1.0 } else { 0.0 },
            if style.shading.per_pixel { 1.0 } else { 0.0 },
            points.len() as f32,
            blobs.len() as f32,
        ];
        self
    }
}

/// The affine correction distance the shader uses. Never zero, so the
/// shader can divide by it; a tiny value is indistinguishable from pure
/// affine mapping.
fn affine_correction(style: &RetroStyle) -> f32 {
    let d = style.affine_uv.correction_distance;
    if d.is_finite() {
        d.clamp(1.0e-4, 1000.0)
    } else {
        1.0e-4
    }
}

impl DrawUniforms {
    pub fn new(model: Mat4, material: &Material) -> Self {
        // Inverse-transpose keeps normals perpendicular under non-uniform
        // scale. A singular matrix (zero scale) falls back to the model
        // matrix itself so no NaNs reach the GPU.
        let normal_matrix = if model.determinant().abs() > 1.0e-12 {
            model.inverse().transpose()
        } else {
            model
        };
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        Self {
            model: model.to_cols_array_2d(),
            normal_matrix: normal_matrix.to_cols_array_2d(),
            tint: material.tint,
            params: [
                material.cutoff(),
                flag(material.lit),
                flag(material.flat_shading),
                flag(material.allow_affine),
            ],
            emissive: [
                material.emissive[0],
                material.emissive[1],
                material.emissive[2],
                0.0,
            ],
            skin: [0.0; 4],
            uv_tile: match material.uv_tile {
                Some([x, y, z]) => [x, y, z, 1.0],
                None => [1.0, 1.0, 1.0, 0.0],
            },
        }
    }
}

impl JointPalette {
    /// Up to [`MAX_JOINTS`] matrices; missing entries are identity.
    pub fn new(joints: &[Mat4]) -> Self {
        let mut out = [Mat4::IDENTITY.to_cols_array_2d(); MAX_JOINTS];
        for (slot, m) in out.iter_mut().zip(joints) {
            if m.is_finite() {
                *slot = m.to_cols_array_2d();
            }
        }
        Self { joints: out }
    }
}

impl CompositeUniforms {
    pub fn new(rect: Rect, source_size: (u32, u32), style: &RetroStyle, output_srgb: bool) -> Self {
        let levels = if style.color_reduction.enabled {
            ((1u32 << style.color_reduction.bits.clamp(1, 8)) - 1) as f32
        } else {
            255.0
        };
        let dither = if style.dither.enabled {
            style.dither.strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            rect: [
                rect.x as f32,
                rect.y as f32,
                rect.width.max(1) as f32,
                rect.height.max(1) as f32,
            ],
            source_size: [source_size.0 as f32, source_size.1 as f32, 0.0, 0.0],
            color: [levels, dither, if output_srgb { 0.0 } else { 1.0 }, 0.0],
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum RenderError {
    #[error("render target {width}x{height} exceeds the device limit of {max} pixels per side")]
    TargetTooLarge { width: u32, height: u32, max: u32 },
}

/// Vertex and index buffers for one validated mesh.
#[derive(Debug)]
pub struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// A sampled texture with its bind group.
#[derive(Debug)]
pub struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// One mesh instance to draw.
#[derive(Debug, Clone, Copy)]
pub struct DrawItem<'a> {
    pub mesh: &'a GpuMesh,
    pub texture: &'a GpuTexture,
    pub material: &'a Material,
    pub model: Mat4,
    /// Joint matrices (mesh space, at most [`MAX_JOINTS`]) for a skinned
    /// mesh. `None` draws the vertices as stored.
    pub joints: Option<&'a [Mat4]>,
}

/// One view's place in the composite output.
#[derive(Debug, Clone, Copy)]
pub struct CompositeSlot<'a> {
    pub view: &'a RenderView,
    /// Where the whole internal image maps to in the output.
    pub rect: Rect,
    /// The part of the output this slot may write (e.g. half for a split view).
    pub scissor: Rect,
    /// Supplies colour reduction and dither settings.
    pub style: &'a RetroStyle,
}

/// Internal-resolution targets and per-view uniform buffers.
///
/// Each view owns its uniforms, so several views can be rendered and
/// composited in one submission (e.g. a split comparison). Rendering the
/// same view twice before a submit overwrites its uniforms.
#[derive(Debug)]
pub struct RenderView {
    size: (u32, u32),
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    frame_buffer: wgpu::Buffer,
    /// One per sampler (see [`sampler_index`]).
    frame_bind_groups: [wgpu::BindGroup; 4],
    joint_buffer: wgpu::Buffer,
    joint_bind_group: wgpu::BindGroup,
    joint_capacity: u32,
    draw_buffer: wgpu::Buffer,
    draw_bind_group: wgpu::BindGroup,
    draw_capacity: u32,
    composite_buffer: wgpu::Buffer,
    composite_bind_group: wgpu::BindGroup,
}

impl RenderView {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The linear scene image (for captures and tests).
    pub fn color_texture(&self) -> &wgpu::Texture {
        &self.color
    }
}

#[derive(Debug)]
pub struct RetroRenderer {
    /// Indexed by `blend * 2 + double_sided`.
    scene_pipelines: [wgpu::RenderPipeline; 4],
    composite_pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    joint_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    /// Indexed by [`sampler_index`].
    samplers: [wgpu::Sampler; 4],
    draw_stride: u64,
    joint_stride: u64,
    output_srgb: bool,
}

const INITIAL_DRAW_CAPACITY: u32 = 64;
const INITIAL_JOINT_CAPACITY: u32 = 4;

/// `linear * 2 + mipmaps`.
fn sampler_index(style: &RetroStyle) -> usize {
    usize::from(style.textures.filter == TextureFilter::Linear) * 2
        + usize::from(style.textures.mipmaps)
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    size: usize,
    dynamic: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size as u64),
        },
        count: None,
    }
}

impl RetroRenderer {
    /// Builds all pipelines. `output_format` is the format of the target
    /// passed to [`RetroRenderer::composite`] (usually the window surface).
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SCENE_SHADER.into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        let vs_fs = wgpu::ShaderStages::VERTEX_FRAGMENT;

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame uniforms"),
            entries: &[
                uniform_entry(0, vs_fs, std::mem::size_of::<FrameUniforms>(), false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let joint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("joint palette"),
            entries: &[uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX,
                std::mem::size_of::<JointPalette>(),
                true,
            )],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw uniforms"),
            entries: &[uniform_entry(
                0,
                vs_fs,
                std::mem::size_of::<DrawUniforms>(),
                true,
            )],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("base texture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                uniform_entry(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    std::mem::size_of::<CompositeUniforms>(),
                    false,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pipeline layout"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&draw_layout),
                Some(&texture_layout),
                Some(&joint_layout),
            ],
            immediate_size: 0,
        });

        let scene_pipeline = |blend: bool, double_sided: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(match (blend, double_sided) {
                    (false, false) => "scene opaque",
                    (false, true) => "scene opaque double-sided",
                    (true, false) => "scene blended",
                    (true, true) => "scene blended double-sided",
                }),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &scene_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(crate::Vertex::layout())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: (!double_sided).then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!blend),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &scene_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_FORMAT,
                        blend: Some(if blend {
                            wgpu::BlendState::ALPHA_BLENDING
                        } else {
                            wgpu::BlendState::REPLACE
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let scene_pipelines = [
            scene_pipeline(false, false),
            scene_pipeline(false, true),
            scene_pipeline(true, false),
            scene_pipeline(true, true),
        ];

        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("composite pipeline layout"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Every texture has a full mip chain; "no mipmaps" clamps sampling
        // to level 0, so styles switch without re-uploading textures.
        let sampler = |linear: bool, mipmaps: bool| {
            let filter = if linear {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("base texture sampler"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: if linear {
                    wgpu::MipmapFilterMode::Linear
                } else {
                    wgpu::MipmapFilterMode::Nearest
                },
                lod_min_clamp: 0.0,
                lod_max_clamp: if mipmaps { 32.0 } else { 0.0 },
                ..Default::default()
            })
        };
        let samplers = [
            sampler(false, false),
            sampler(false, true),
            sampler(true, false),
            sampler(true, true),
        ];

        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let draw_stride = (std::mem::size_of::<DrawUniforms>() as u64).div_ceil(align) * align;
        let joint_stride = (std::mem::size_of::<JointPalette>() as u64).div_ceil(align) * align;

        Self {
            scene_pipelines,
            composite_pipeline,
            frame_layout,
            draw_layout,
            texture_layout,
            joint_layout,
            composite_layout,
            samplers,
            draw_stride,
            joint_stride,
            output_srgb: output_format.is_srgb(),
        }
    }

    pub fn upload_mesh(
        &self,
        device: &wgpu::Device,
        mesh: &MeshData,
    ) -> Result<GpuMesh, MeshError> {
        mesh.validate()?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
        })
    }

    pub fn upload_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &TextureData,
    ) -> Result<GpuTexture, TextureError> {
        data.validate(device.limits().max_texture_dimension_2d)?;
        let mut bytes = data.rgba.clone();
        for level in data.mip_chain() {
            bytes.extend_from_slice(&level.rgba);
        }
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("base texture"),
                size: wgpu::Extent3d {
                    width: data.width,
                    height: data.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: data.mip_level_count(),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &bytes,
        );
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("base texture"),
            layout: &self.texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Ok(GpuTexture { bind_group })
    }

    /// Creates a view with `width`×`height` internal targets.
    pub fn create_view(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<RenderView, RenderError> {
        let (width, height) = (width.max(1), height.max(1));
        let max = device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            return Err(RenderError::TargetTooLarge { width, height, max });
        }
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind_groups = self.samplers.each_ref().map(|sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frame uniforms"),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: frame_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });
        let (draw_buffer, draw_bind_group) = self.create_draw_buffer(device, INITIAL_DRAW_CAPACITY);
        let (joint_buffer, joint_bind_group) =
            self.create_joint_buffer(device, INITIAL_JOINT_CAPACITY);

        let composite_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite uniforms"),
            size: std::mem::size_of::<CompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: composite_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
            ],
        });

        Ok(RenderView {
            size: (width, height),
            color,
            color_view,
            depth_view,
            frame_buffer,
            frame_bind_groups,
            joint_buffer,
            joint_bind_group,
            joint_capacity: INITIAL_JOINT_CAPACITY,
            draw_buffer,
            draw_bind_group,
            draw_capacity: INITIAL_DRAW_CAPACITY,
            composite_buffer,
            composite_bind_group,
        })
    }

    /// Recreates the view's targets only when the size changes.
    pub fn ensure_view_size(
        &self,
        device: &wgpu::Device,
        view: &mut RenderView,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        if view.size != (width.max(1), height.max(1)) {
            *view = self.create_view(device, width, height)?;
        }
        Ok(())
    }

    fn create_draw_buffer(
        &self,
        device: &wgpu::Device,
        capacity: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draw uniforms"),
            size: self.draw_stride * u64::from(capacity),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw uniforms"),
            layout: &self.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniforms>() as u64),
                }),
            }],
        });
        (buffer, bind_group)
    }

    fn create_joint_buffer(
        &self,
        device: &wgpu::Device,
        capacity: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("joint palettes"),
            size: self.joint_stride * u64::from(capacity.max(1)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("joint palettes"),
            layout: &self.joint_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<JointPalette>() as u64),
                }),
            }],
        });
        (buffer, bind_group)
    }

    /// Records the scene pass for `view`: clears to the style's background,
    /// draws opaque/cutout items in order, then blended items back-to-front.
    #[allow(clippy::too_many_arguments)]
    pub fn render_view(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &mut RenderView,
        camera: &Camera,
        lighting: &Lighting,
        style: &RetroStyle,
        items: &[DrawItem<'_>],
    ) {
        let frame = FrameUniforms::new(camera, lighting, style, view.size);
        queue.write_buffer(&view.frame_buffer, 0, bytemuck::bytes_of(&frame));

        let order = draw_order(items, camera.position);
        let count = u32::try_from(order.len()).unwrap_or(u32::MAX);
        if count > view.draw_capacity {
            let capacity = count.next_power_of_two();
            let (buffer, bind_group) = self.create_draw_buffer(device, capacity);
            view.draw_buffer = buffer;
            view.draw_bind_group = bind_group;
            view.draw_capacity = capacity;
        }
        // Joint palette slot per draw (unskinned draws share slot 0 and
        // never read it).
        let mut joint_slots = vec![0u32; order.len()];
        let mut palettes: Vec<JointPalette> = Vec::new();
        for (slot, &i) in order.iter().enumerate() {
            if let Some(joints) = items[i].joints {
                joint_slots[slot] = palettes.len() as u32;
                palettes.push(JointPalette::new(joints));
            }
        }
        let joint_count = u32::try_from(palettes.len()).unwrap_or(u32::MAX);
        if joint_count > view.joint_capacity {
            let capacity = joint_count.next_power_of_two();
            let (buffer, bind_group) = self.create_joint_buffer(device, capacity);
            view.joint_buffer = buffer;
            view.joint_bind_group = bind_group;
            view.joint_capacity = capacity;
        }
        if !palettes.is_empty() {
            let stride = self.joint_stride as usize;
            let size = std::mem::size_of::<JointPalette>();
            let mut bytes = vec![0u8; stride * palettes.len()];
            for (slot, p) in palettes.iter().enumerate() {
                bytes[slot * stride..slot * stride + size].copy_from_slice(bytemuck::bytes_of(p));
            }
            queue.write_buffer(&view.joint_buffer, 0, &bytes);
        }
        if !order.is_empty() {
            let stride = self.draw_stride as usize;
            let mut bytes = vec![0u8; stride * order.len()];
            for (slot, &i) in order.iter().enumerate() {
                let mut u = DrawUniforms::new(items[i].model, items[i].material);
                if items[i].joints.is_some() {
                    u.skin[0] = 1.0;
                }
                bytes[slot * stride..slot * stride + std::mem::size_of::<DrawUniforms>()]
                    .copy_from_slice(bytemuck::bytes_of(&u));
            }
            queue.write_buffer(&view.draw_buffer, 0, &bytes);
        }

        let [r, g, b] = srgb8_to_linear(style.atmosphere.background);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(r),
                        g: f64::from(g),
                        b: f64::from(b),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &view.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &view.frame_bind_groups[sampler_index(style)], &[]);
        for (slot, &i) in order.iter().enumerate() {
            let item = &items[i];
            let pipeline = usize::from(item.material.is_blended()) * 2
                + usize::from(item.material.double_sided);
            pass.set_pipeline(&self.scene_pipelines[pipeline]);
            let offset = (slot as u64 * self.draw_stride) as u32;
            pass.set_bind_group(1, &view.draw_bind_group, &[offset]);
            pass.set_bind_group(2, &item.texture.bind_group, &[]);
            let joint_offset = (u64::from(joint_slots[slot]) * self.joint_stride) as u32;
            pass.set_bind_group(3, &view.joint_bind_group, &[joint_offset]);
            pass.set_vertex_buffer(0, item.mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(item.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..item.mesh.index_count, 0, 0..1);
        }
    }

    /// Records the composite pass: clears `output` to black and draws each
    /// slot's view into its rectangle, limited to its scissor. Each view may
    /// appear at most once per call.
    pub fn composite(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        output_size: (u32, u32),
        slots: &[CompositeSlot<'_>],
    ) {
        for slot in slots {
            let u = CompositeUniforms::new(slot.rect, slot.view.size, slot.style, self.output_srgb);
            queue.write_buffer(&slot.view.composite_buffer, 0, bytemuck::bytes_of(&u));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        for slot in slots {
            let Some(scissor) = clip_rect(slot.scissor, slot.rect, output_size) else {
                continue;
            };
            pass.set_scissor_rect(scissor.x, scissor.y, scissor.width, scissor.height);
            pass.set_bind_group(0, &slot.view.composite_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Opaque/cutout items in submission order, then blended items far to near.
fn draw_order(items: &[DrawItem<'_>], eye: Vec3) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len())
        .filter(|&i| !items[i].material.is_blended())
        .collect();
    let mut blended: Vec<(usize, f32)> = (0..items.len())
        .filter(|&i| items[i].material.is_blended())
        .map(|i| (i, items[i].model.w_axis.truncate().distance_squared(eye)))
        .collect();
    blended.sort_by(|a, b| b.1.total_cmp(&a.1));
    order.extend(blended.into_iter().map(|(i, _)| i));
    order
}

/// Intersection of `a`, `b` and the output area, or `None` if empty.
fn clip_rect(a: Rect, b: Rect, output: (u32, u32)) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width).min(output.0);
    let y1 = (a.y + a.height).min(b.y + b.height).min(output.1);
    (x1 > x0 && y1 > y0).then(|| Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::AlphaMode;

    fn parse(source: &str) -> naga::Module {
        naga::front::wgsl::parse_str(source).expect("shader must parse")
    }

    fn validate(source: &str) {
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&parse(source))
        .expect("shader must validate with no optional capabilities");
    }

    #[test]
    fn shaders_validate() {
        validate(SCENE_SHADER);
        validate(COMPOSITE_SHADER);
    }

    fn check_layout(source: &str, name: &str, size: usize, fields: &[(&str, usize)]) {
        let module = parse(source);
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} in shader"));
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("{name} is not a struct");
        };
        assert_eq!(*span as usize, size, "size of {name}");
        assert_eq!(members.len(), fields.len(), "member count of {name}");
        for (member, (field, offset)) in members.iter().zip(fields) {
            assert_eq!(member.name.as_deref(), Some(*field));
            assert_eq!(member.offset as usize, *offset, "offset of {name}.{field}");
        }
    }

    #[test]
    fn uniform_layouts_match_wgsl() {
        use std::mem::{offset_of, size_of};
        check_layout(
            SCENE_SHADER,
            "FrameUniforms",
            size_of::<FrameUniforms>(),
            &[
                ("view_proj", offset_of!(FrameUniforms, view_proj)),
                ("camera_pos", offset_of!(FrameUniforms, camera_pos)),
                ("light_dir", offset_of!(FrameUniforms, light_dir)),
                ("light_color", offset_of!(FrameUniforms, light_color)),
                ("ambient", offset_of!(FrameUniforms, ambient)),
                ("fog_color", offset_of!(FrameUniforms, fog_color)),
                ("fog_range", offset_of!(FrameUniforms, fog_range)),
                ("snap", offset_of!(FrameUniforms, snap)),
                ("options", offset_of!(FrameUniforms, options)),
                ("shadow_params", offset_of!(FrameUniforms, shadow_params)),
                ("lights", offset_of!(FrameUniforms, lights)),
                ("blobs", offset_of!(FrameUniforms, blobs)),
            ],
        );
        check_layout(
            SCENE_SHADER,
            "DrawUniforms",
            size_of::<DrawUniforms>(),
            &[
                ("model", offset_of!(DrawUniforms, model)),
                ("normal_matrix", offset_of!(DrawUniforms, normal_matrix)),
                ("tint", offset_of!(DrawUniforms, tint)),
                ("params", offset_of!(DrawUniforms, params)),
                ("emissive", offset_of!(DrawUniforms, emissive)),
                ("skin", offset_of!(DrawUniforms, skin)),
                ("uv_tile", offset_of!(DrawUniforms, uv_tile)),
            ],
        );
        check_layout(
            SCENE_SHADER,
            "JointPalette",
            size_of::<JointPalette>(),
            &[("joints", offset_of!(JointPalette, joints))],
        );
        check_layout(
            COMPOSITE_SHADER,
            "CompositeUniforms",
            size_of::<CompositeUniforms>(),
            &[
                ("rect", offset_of!(CompositeUniforms, rect)),
                ("source_size", offset_of!(CompositeUniforms, source_size)),
                ("color", offset_of!(CompositeUniforms, color)),
            ],
        );
    }

    #[test]
    fn vertex_inputs_match_rust_attributes() {
        let module = parse(SCENE_SHADER);
        let vs = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_main")
            .expect("vs_main");
        let arg_ty = &module.types[vs.function.arguments[0].ty];
        let naga::TypeInner::Struct { members, .. } = &arg_ty.inner else {
            panic!("vertex input is not a struct");
        };
        let locations: Vec<u32> = members
            .iter()
            .filter_map(|m| match m.binding {
                Some(naga::Binding::Location { location, .. }) => Some(location),
                _ => None,
            })
            .collect();
        let rust: Vec<u32> = crate::Vertex::ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, rust);
    }

    #[test]
    fn affine_varying_is_depth_weighted() {
        let module = parse(SCENE_SHADER);
        let vs = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_main")
            .unwrap();
        let out_ty = &module.types[vs.function.result.as_ref().unwrap().ty];
        let naga::TypeInner::Struct { members, .. } = &out_ty.inner else {
            panic!("vertex output is not a struct");
        };
        let interp = |name: &str| {
            let m = members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .unwrap();
            match m.binding {
                Some(naga::Binding::Location { interpolation, .. }) => interpolation,
                _ => panic!("{name} has no location"),
            }
        };
        // Affine mapping comes from the weights in `affine_weight`, so the
        // varying itself is perspective-interpolated (and clipped correctly).
        assert_eq!(interp("uv_affine"), Some(naga::Interpolation::Perspective));
        assert_eq!(
            interp("uv_perspective"),
            Some(naga::Interpolation::Perspective)
        );
        assert_eq!(interp("color_flat"), Some(naga::Interpolation::Flat));
    }

    #[test]
    fn frame_uniforms_follow_style_toggles() {
        let cam = Camera::default();
        let mut style = RetroStyle::ps1();
        let u = FrameUniforms::new(&cam, &Lighting::default(), &style, (320, 240));
        assert_eq!(u.snap, [320.0, 240.0, 1.0, 1.0]);
        assert_eq!(u.options[0], 1.0);
        assert_eq!(u.shadow_params[1], 2.0);
        assert_eq!(u.fog_color[3], 1.0);
        style.vertex_snap.enabled = false;
        style.affine_uv.enabled = false;
        style.fog.enabled = false;
        let u = FrameUniforms::new(&cam, &Lighting::default(), &style, (320, 240));
        assert_eq!(u.snap[3], 0.0);
        assert_eq!(u.options[0], 0.0);
        assert_eq!(u.fog_color[3], 0.0);
        let d = glam::Vec3::from_slice(&u.light_dir[..3]);
        assert!((d.length() - 1.0).abs() < 1e-5 && d.y > 0.0);
    }

    #[test]
    fn local_lights_and_blobs_are_bounded_and_nearest_first() {
        let cam = Camera::default();
        let light = |x: f32| PointLight {
            position: cam.position + Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            range: 1.0,
        };
        let mut lighting = Lighting {
            points: (0..20).map(|i| light(20.0 - i as f32)).collect(),
            blobs: vec![
                BlobShadow {
                    bottom: Vec3::ZERO,
                    radius: f32::NAN,
                },
                BlobShadow {
                    bottom: Vec3::ONE,
                    radius: 0.5,
                },
            ],
            ..Lighting::default()
        };
        lighting.points.push(PointLight {
            range: 0.0,
            ..light(0.0)
        });
        let mut style = RetroStyle::ps2();
        let u = FrameUniforms::new(&cam, &lighting, &style, (64, 64));
        assert_eq!(u.options, [0.0, 1.0, MAX_LOCAL_LIGHTS as f32, 1.0]);
        // Nearest first; the zero-range light is ignored.
        assert_eq!(u.lights[0][0], cam.position.x + 1.0);
        assert_eq!(u.lights[0][3], 1.0);
        assert_eq!(u.blobs[0], [1.0, 1.0, 1.0, 0.5]);
        style.shading.local_lights = false;
        style.shadows.enabled = false;
        let u = FrameUniforms::new(&cam, &lighting, &style, (64, 64));
        assert_eq!(&u.options[2..], &[0.0, 0.0]);
        assert_eq!(sampler_index(&RetroStyle::ps1()), 0);
        assert_eq!(sampler_index(&RetroStyle::ps2()), 3);
    }

    #[test]
    fn composite_uniforms_levels_and_dither() {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let mut style = RetroStyle::ps1();
        let u = CompositeUniforms::new(rect, (10, 10), &style, true);
        assert_eq!(u.color, [31.0, 1.0, 0.0, 0.0]);
        style.color_reduction.enabled = false;
        style.dither.enabled = false;
        let u = CompositeUniforms::new(rect, (10, 10), &style, false);
        assert_eq!(u.color, [255.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn draw_uniforms_material_params_and_singular_model() {
        let m = Material {
            alpha: AlphaMode::Cutout { threshold: 2.0 },
            lit: false,
            flat_shading: true,
            allow_affine: false,
            ..Material::default()
        };
        let u = DrawUniforms::new(Mat4::from_scale(Vec3::ZERO), &m);
        assert_eq!(u.params, [1.0, 0.0, 1.0, 0.0]);
        assert!(u.normal_matrix.iter().flatten().all(|v| v.is_finite()));
        let u = DrawUniforms::new(Mat4::IDENTITY, &Material::default());
        assert_eq!(u.params, [-1.0, 1.0, 0.0, 1.0]);
        assert_eq!(u.emissive, [0.0; 4]);
        let glow = Material {
            emissive: [0.25, 0.5, 1.0],
            ..Material::default()
        };
        assert_eq!(
            DrawUniforms::new(Mat4::IDENTITY, &glow).emissive,
            [0.25, 0.5, 1.0, 0.0]
        );
    }

    #[test]
    fn clip_rect_intersects_with_output() {
        let a = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = Rect {
            x: 50,
            y: 10,
            width: 100,
            height: 20,
        };
        assert_eq!(
            clip_rect(a, b, (80, 80)),
            Some(Rect {
                x: 50,
                y: 10,
                width: 30,
                height: 20
            })
        );
        assert_eq!(clip_rect(a, b, (40, 80)), None);
    }
}
