//! Basic surface materials.
//!
//! A material describes how a mesh is shaded; the base texture is passed
//! separately so several materials can share one texture.
//!
//! Ordering: opaque and cutout surfaces are drawn first, in submission order,
//! with depth testing and depth writes. Blended surfaces follow, sorted
//! back-to-front by the distance from the camera to their model origin, with
//! depth testing but no depth writes. Sorting is per object, not per
//! triangle, so intersecting or self-overlapping transparent meshes can
//! still show ordering errors.

/// How a surface's alpha is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaMode {
    /// Alpha ignored; fully opaque.
    Opaque,
    /// Fragments with alpha below `threshold` are discarded (no blending,
    /// depth written for the rest). `threshold` is clamped to `[0, 1]`.
    Cutout { threshold: f32 },
    /// Conventional (non-premultiplied) alpha blending.
    Blend,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Linear RGBA multiplied with the texture and vertex colour.
    pub tint: [f32; 4],
    pub alpha: AlphaMode,
    /// Apply ambient + directional lighting per vertex. Unlit surfaces show
    /// texture × vertex colour × tint.
    pub lit: bool,
    /// Use the first vertex's shading for the whole triangle instead of
    /// interpolating (faceted look). Normals in the mesh are not modified.
    pub flat_shading: bool,
    /// Whether this surface uses affine UVs when the style enables them.
    /// Turn off for surfaces that must never warp (e.g. text decals).
    pub allow_affine: bool,
    /// Draw both faces (no back-face culling).
    pub double_sided: bool,
    /// Linear RGB added after lighting (glTF emissive factor). Not lit, but
    /// fogged like everything else.
    pub emissive: [f32; 3],
}

impl Default for Material {
    fn default() -> Self {
        Self {
            tint: [1.0; 4],
            alpha: AlphaMode::Opaque,
            lit: true,
            flat_shading: false,
            allow_affine: true,
            double_sided: false,
            emissive: [0.0; 3],
        }
    }
}

impl Material {
    pub fn is_blended(&self) -> bool {
        matches!(self.alpha, AlphaMode::Blend)
    }

    /// Value of the shader's alpha cutoff parameter (negative = none).
    pub(crate) fn cutoff(&self) -> f32 {
        match self.alpha {
            AlphaMode::Cutout { threshold } if threshold.is_finite() => threshold.clamp(0.0, 1.0),
            AlphaMode::Cutout { .. } => 0.5,
            _ => -1.0,
        }
    }
}
