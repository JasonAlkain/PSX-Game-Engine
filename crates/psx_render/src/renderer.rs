//! Forward mesh renderer.
//!
//! GPU objects (pipeline, layouts, sampler) are created once in
//! [`MeshRenderer::new`]; per-frame work is limited to a uniform write and a
//! render pass.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{MeshData, MeshError, TextureData, TextureError};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SHADER_SOURCE: &str = include_str!("shaders/basic.wgsl");

/// Per-frame uniforms. Must match `FrameUniforms` in `shaders/basic.wgsl`;
/// `tests::uniform_layout_matches_wgsl` checks size and field offsets.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub model: [[f32; 4]; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient: [f32; 4],
}

/// Simple directional + ambient lighting, all colours linear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// Direction the light travels *from* the light towards the scene.
    pub direction: Vec3,
    pub color: Vec3,
    pub ambient: Vec3,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: Vec3::splat(0.85),
            ambient: Vec3::splat(0.25),
        }
    }
}

impl FrameUniforms {
    pub fn new(view_proj: Mat4, model: Mat4, lighting: &Lighting) -> Self {
        let to_light = (-lighting.direction).normalize_or(Vec3::Y);
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            light_dir: to_light.extend(0.0).to_array(),
            light_color: lighting.color.extend(1.0).to_array(),
            ambient: lighting.ambient.extend(1.0).to_array(),
        }
    }
}

/// Depth buffer matching a colour target's size.
#[derive(Debug)]
pub struct DepthTarget {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl DepthTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = (width.max(1), height.max(1));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&Default::default()),
            size,
        }
    }

    /// Recreates the buffer only when the size actually changed.
    pub fn ensure_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if (width.max(1), height.max(1)) != self.size {
            *self = Self::new(device, width, height);
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
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

#[derive(Debug)]
pub struct MeshRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl MeshRenderer {
    /// Builds the pipeline for a colour target of `color_format`.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("basic.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<FrameUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("base texture"),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(crate::Vertex::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame uniforms"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Nearest sampling, no mipmaps: crisp texels for the retro look.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            pipeline,
            uniform_buffer,
            frame_bind_group,
            texture_layout,
            sampler,
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
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("base texture"),
                size: wgpu::Extent3d {
                    width: data.width,
                    height: data.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data.rgba,
        );
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("base texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Ok(GpuTexture { bind_group })
    }

    /// Records a pass that clears `color`/`depth` and draws `mesh`.
    ///
    /// `clear_color` is linear RGB.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &DepthTarget,
        clear_color: wgpu::Color,
        uniforms: &FrameUniforms,
        mesh: &GpuMesh,
        texture: &GpuTexture,
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(uniforms));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth.view(),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.frame_bind_group, &[]);
        pass.set_bind_group(1, &texture.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_shader() -> naga::Module {
        naga::front::wgsl::parse_str(SHADER_SOURCE).expect("basic.wgsl must parse")
    }

    #[test]
    fn shader_validates() {
        let module = parse_shader();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("basic.wgsl must validate with no optional capabilities");
    }

    #[test]
    fn uniform_layout_matches_wgsl() {
        let module = parse_shader();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("FrameUniforms"))
            .expect("FrameUniforms struct in shader");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("FrameUniforms is not a struct");
        };
        assert_eq!(*span as usize, std::mem::size_of::<FrameUniforms>());

        let rust_offsets = [
            ("view_proj", std::mem::offset_of!(FrameUniforms, view_proj)),
            ("model", std::mem::offset_of!(FrameUniforms, model)),
            ("light_dir", std::mem::offset_of!(FrameUniforms, light_dir)),
            (
                "light_color",
                std::mem::offset_of!(FrameUniforms, light_color),
            ),
            ("ambient", std::mem::offset_of!(FrameUniforms, ambient)),
        ];
        assert_eq!(members.len(), rust_offsets.len());
        for (member, (name, offset)) in members.iter().zip(rust_offsets) {
            assert_eq!(member.name.as_deref(), Some(name));
            assert_eq!(member.offset as usize, offset, "offset of {name}");
        }
    }

    #[test]
    fn vertex_inputs_match_rust_attributes() {
        let module = parse_shader();
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
    fn light_direction_is_normalized_towards_light() {
        let u = FrameUniforms::new(Mat4::IDENTITY, Mat4::IDENTITY, &Lighting::default());
        let d = Vec3::from_slice(&u.light_dir[..3]);
        assert!((d.length() - 1.0).abs() < 1e-5);
        assert!(d.y > 0.0, "light from above points up towards the light");
    }
}
