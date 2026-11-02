//! CPU-side mesh data and validation.
//!
//! Front faces wind counter-clockwise when viewed from outside, matching
//! `wgpu::FrontFace::Ccw`. Texture coordinates have their origin at the
//! top-left of the image, +V pointing down (the wgpu/glTF convention).

use bytemuck::{Pod, Zeroable};
use thiserror::Error;

/// Vertex layout shared by the WGSL `VertexInput` struct in `shaders/basic.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA vertex color, multiplied with the texture.
    pub color: [f32; 4],
}

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
    ];

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
                .chain(&v.color);
            if !all.into_iter().all(|f| f.is_finite()) {
                return Err(MeshError::NonFiniteVertex(i));
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
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn vertex_layout_matches_struct_size() {
        assert_eq!(std::mem::size_of::<Vertex>(), 48);
        assert_eq!(Vertex::layout().array_stride, 48);
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
