//! CPU-side mesh data and validation.
//!
//! Front faces wind counter-clockwise when viewed from outside, matching
//! `wgpu::FrontFace::Ccw`. Texture coordinates have their origin at the
//! top-left of the image, +V pointing down (the wgpu/glTF convention).

use bytemuck::{Pod, Zeroable};
use thiserror::Error;

/// Vertex layout shared by the WGSL `VertexInput` struct in `shaders/scene.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA vertex color, multiplied with the texture.
    pub color: [f32; 4],
    /// Up to four joints (indices into the draw's joint palette) for
    /// skinned meshes. Ignored unless the draw supplies joints.
    pub joints: [u16; 4],
    /// Weights of `joints`, summing to 1 for skinned meshes.
    pub weights: [f32; 4],
}

/// Most joints in one skinned draw's palette.
pub const MAX_JOINTS: usize = 64;

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
        4 => Uint16x4,
        5 => Float32x4,
    ];

    /// An unskinned vertex.
    pub fn new(position: [f32; 3], normal: [f32; 3], uv: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            position,
            normal,
            uv,
            color,
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum MeshError {
    #[error("mesh has no triangles")]
    Empty,
    #[error("index count {0} is not a multiple of 3")]
    IndexCountNotTriangles(usize),
    #[error("index {index} at position {position} is out of range for {vertex_count} vertices")]
    IndexOutOfRange {
        position: usize,
        index: u32,
        vertex_count: usize,
    },
    #[error("vertex {0} contains a non-finite value")]
    NonFiniteVertex(usize),
    #[error("vertex {vertex} uses joint {joint}; at most {MAX_JOINTS} joints are supported")]
    JointOutOfRange { vertex: usize, joint: u16 },
    #[error("mesh has {0} vertices, more than a u32 index can address")]
    TooManyVertices(usize),
}

/// (outward normal, right axis, up axis, tint) for one cube face.
type CubeFace = ([f32; 3], [f32; 3], [f32; 3], [f32; 4]);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Checks the invariants the GPU upload relies on.
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.indices.is_empty() || self.vertices.is_empty() {
            return Err(MeshError::Empty);
        }
        if self.vertices.len() > u32::MAX as usize {
            return Err(MeshError::TooManyVertices(self.vertices.len()));
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshError::IndexCountNotTriangles(self.indices.len()));
        }
        for (position, &index) in self.indices.iter().enumerate() {
            if index as usize >= self.vertices.len() {
                return Err(MeshError::IndexOutOfRange {
                    position,
                    index,
                    vertex_count: self.vertices.len(),
                });
            }
        }
        for (i, v) in self.vertices.iter().enumerate() {
            let all = v
                .position
                .iter()
                .chain(&v.normal)
                .chain(&v.uv)
                .chain(&v.color)
                .chain(&v.weights);
            if !all.into_iter().all(|f| f.is_finite()) {
                return Err(MeshError::NonFiniteVertex(i));
            }
            if let Some(&joint) = v.joints.iter().find(|&&j| usize::from(j) >= MAX_JOINTS) {
                return Err(MeshError::JointOutOfRange { vertex: i, joint });
            }
        }
        Ok(())
    }

    /// Axis-aligned cube centred on the origin with edge length `size`.
    ///
    /// Each face has its own four vertices so normals stay flat and every face
    /// maps the full texture. Faces are tinted slightly so orientation is easy
    /// to read in the test scene.
    pub fn cube(size: f32) -> Self {
        let h = size * 0.5;
        // (normal, right, up, tint): the face's outward normal and the two axes
        // spanning it, chosen so right x up == normal (counter-clockwise front).
        let faces: [CubeFace; 6] = [
            (
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 1.0],
            ),
            (
                [0.0, 0.0, -1.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.8, 0.8, 1.0],
            ),
            (
                [1.0, 0.0, 0.0],
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
                [0.8, 1.0, 0.8, 1.0],
            ),
            (
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [0.8, 0.8, 1.0, 1.0],
            ),
            (
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, -1.0],
                [1.0, 1.0, 0.8, 1.0],
            ),
            (
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.8, 1.0, 1.0, 1.0],
            ),
        ];

        let mut mesh = MeshData::default();
        for (normal, right, up, color) in faces {
            let base = mesh.vertices.len() as u32;
            // Corners: bottom-left, bottom-right, top-right, top-left.
            for (sx, sy, uv) in [
                (-1.0, -1.0, [0.0, 1.0]),
                (1.0, -1.0, [1.0, 1.0]),
                (1.0, 1.0, [1.0, 0.0]),
                (-1.0, 1.0, [0.0, 0.0]),
            ] {
                let position =
                    std::array::from_fn(|i| (normal[i] + sx * right[i] + sy * up[i]) * h);
                mesh.vertices.push(Vertex {
                    position,
                    normal,
                    uv,
                    color,
                    joints: [0; 4],
                    weights: [0.0; 4],
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh
    }
}

impl MeshData {
    /// Rectangle in the XY plane centred on the origin, facing +Z, with UVs
    /// covering the texture once (top-left = (0, 0)).
    pub fn quad(width: f32, height: f32) -> Self {
        let (w, h) = (width * 0.5, height * 0.5);
        let n = [0.0, 0.0, 1.0];
        let c = [1.0; 4];
        let v = |x: f32, y: f32, u: f32, v: f32| Vertex {
            position: [x, y, 0.0],
            normal: n,
            uv: [u, v],
            color: c,
            joints: [0; 4],
            weights: [0.0; 4],
        };
        MeshData {
            vertices: vec![
                v(-w, -h, 0.0, 1.0),
                v(w, -h, 1.0, 1.0),
                v(w, h, 1.0, 0.0),
                v(-w, h, 0.0, 0.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    /// Horizontal grid in the XZ plane centred on the origin, facing +Y,
    /// split into `divisions.0`×`divisions.1` quads (along X and Z). UVs span
    /// `0..uv_repeat.0` along +X and `0..uv_repeat.1` along +Z.
    ///
    /// Retro hardware relied on tessellation to limit affine warping, so
    /// floors are built from several quads rather than one.
    pub fn plane(size_x: f32, size_z: f32, divisions: (u32, u32), uv_repeat: (f32, f32)) -> Self {
        let (dx, dz) = (divisions.0.max(1), divisions.1.max(1));
        let mut mesh = MeshData::default();
        for j in 0..=dz {
            for i in 0..=dx {
                let (fx, fz) = (i as f32 / dx as f32, j as f32 / dz as f32);
                mesh.vertices.push(Vertex {
                    position: [(fx - 0.5) * size_x, 0.0, (fz - 0.5) * size_z],
                    normal: [0.0, 1.0, 0.0],
                    uv: [fx * uv_repeat.0, fz * uv_repeat.1],
                    color: [1.0; 4],
                    joints: [0; 4],
                    weights: [0.0; 4],
                });
            }
        }
        let row = dx + 1;
        for j in 0..dz {
            for i in 0..dx {
                let a = j * row + i;
                let (b, c, e) = (a + 1, a + row + 1, a + row);
                // Counter-clockwise seen from above (+Y).
                mesh.indices.extend_from_slice(&[a, e, c, a, c, b]);
            }
        }
        mesh
    }

    /// UV sphere with smooth normals. `segments` around Y, `rings` from pole
    /// to pole.
    pub fn sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let (segments, rings) = (segments.max(3), rings.max(2));
        let mut mesh = MeshData::default();
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let phi = v * std::f32::consts::PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let theta = u * std::f32::consts::TAU;
                let n = [phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin()];
                mesh.vertices.push(Vertex {
                    position: n.map(|c| c * radius),
                    normal: n,
                    uv: [u, v],
                    color: [1.0; 4],
                    joints: [0; 4],
                    weights: [0.0; 4],
                });
            }
        }
        let row = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let a = r * row + s;
                let (b, c, d) = (a + row, a + row + 1, a + 1);
                if r != 0 {
                    mesh.indices.extend_from_slice(&[a, b, d]);
                }
                if r != rings - 1 {
                    mesh.indices.extend_from_slice(&[d, b, c]);
                }
            }
        }
        mesh
    }

    /// Sets every vertex colour (linear RGBA).
    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        for v in &mut self.vertices {
            v.color = color;
        }
        self
    }

    /// Applies `f` to every vertex (e.g. to bake a gradient into vertex colours).
    pub fn map_vertices(mut self, f: impl Fn(&mut Vertex)) -> Self {
        self.vertices.iter_mut().for_each(f);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn vertex_layout_matches_struct_size() {
        assert_eq!(std::mem::size_of::<Vertex>(), 72);
        assert_eq!(Vertex::layout().array_stride, 72);
        assert_eq!(Vertex::ATTRIBUTES[4].offset, 48);
        assert_eq!(Vertex::ATTRIBUTES[5].offset, 56);
        let mut m = MeshData::cube(1.0);
        m.vertices[3].joints[2] = MAX_JOINTS as u16;
        assert_eq!(
            m.validate(),
            Err(MeshError::JointOutOfRange {
                vertex: 3,
                joint: MAX_JOINTS as u16
            })
        );
    }

    #[test]
    fn cube_is_valid() {
        let cube = MeshData::cube(1.0);
        assert_eq!(cube.vertices.len(), 24);
        assert_eq!(cube.indices.len(), 36);
        assert_eq!(cube.validate(), Ok(()));
    }

    #[test]
    fn cube_triangles_wind_counter_clockwise_facing_outward() {
        let cube = MeshData::cube(2.0);
        for tri in cube.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(cube.vertices[tri[i] as usize].position));
            let geometric = (b - a).cross(c - a).normalize();
            let authored = Vec3::from(cube.vertices[tri[0] as usize].normal);
            assert!(
                geometric.dot(authored) > 0.999,
                "triangle {tri:?} winds clockwise"
            );
            // Outward: the face centre lies along the normal.
            assert!(((a + b + c) / 3.0).dot(authored) > 0.0);
        }
    }

    fn assert_ccw_matches_normals(mesh: &MeshData) {
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(mesh.vertices[tri[i] as usize].position));
            let geometric = (b - a).cross(c - a);
            if geometric.length() < 1e-9 {
                continue;
            }
            let authored: Vec3 = tri
                .iter()
                .map(|&i| Vec3::from(mesh.vertices[i as usize].normal))
                .sum();
            assert!(
                geometric.dot(authored) > 0.0,
                "triangle {tri:?} winds clockwise"
            );
        }
    }

    #[test]
    fn generated_shapes_are_valid_and_wind_outward() {
        for mesh in [
            MeshData::quad(2.0, 1.0),
            MeshData::plane(4.0, 6.0, (3, 5), (2.0, 1.0)),
            MeshData::sphere(1.0, 12, 8),
        ] {
            assert_eq!(mesh.validate(), Ok(()));
            assert_ccw_matches_normals(&mesh);
        }
        let plane = MeshData::plane(4.0, 4.0, (4, 2), (1.0, 1.0));
        assert_eq!(plane.vertices.len(), 5 * 3);
        assert_eq!(plane.indices.len(), 4 * 2 * 6);
    }

    #[test]
    fn validation_rejects_bad_meshes() {
        let good = MeshData::cube(1.0);

        let mut m = good.clone();
        m.indices.pop();
        assert_eq!(m.validate(), Err(MeshError::IndexCountNotTriangles(35)));

        let mut m = good.clone();
        m.indices[5] = 99;
        assert!(matches!(
            m.validate(),
            Err(MeshError::IndexOutOfRange { index: 99, .. })
        ));

        let mut m = good.clone();
        m.vertices[3].uv[0] = f32::NAN;
        assert_eq!(m.validate(), Err(MeshError::NonFiniteVertex(3)));

        assert_eq!(MeshData::default().validate(), Err(MeshError::Empty));
    }
}
