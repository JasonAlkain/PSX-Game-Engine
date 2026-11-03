//! Import results and the source-path boundary rules shared by importers.
//!
//! Imported data is plain CPU data with no GPU types. Vertex layout matches
//! the renderer's vertex (position, normal, UV0, linear RGBA colour, four
//! joints and weights) so the upload is a straight copy.
//!
//! Imports never fetch network resources. External files referenced by a
//! model must resolve inside the import boundary (the project directory).

pub mod audio;
pub mod gltf;
pub mod texture;

use std::path::{Path, PathBuf};

use psx_scene::AlphaSetting;
use thiserror::Error;

/// Bumped whenever an importer's output changes for the same input. Part of
/// an import's identity together with the source content and settings.
///
/// - 1: static glTF, PNG/JPEG, WAV/Ogg.
/// - 2: glTF skins and animation clips.
pub const IMPORTER_VERSION: u32 = 2;

/// Largest source file read by an importer.
pub const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest decoded image accepted (per side) before downscaling.
pub const MAX_IMAGE_DIMENSION: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA.
    pub color: [f32; 4],
    /// Joint indices into [`ImportedModel::skeleton`] (skinned parts only).
    pub joints: [u16; 4],
    /// Weights of `joints`, summing to 1 on skinned parts, zero otherwise.
    pub weights: [f32; 4],
}

impl MeshVertex {
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
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuMesh {
    pub vertices: Vec<MeshVertex>,
    /// Triangle list, counter-clockwise front faces.
    pub indices: Vec<u32>,
}

impl CpuMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Tightly packed sRGB RGBA8 pixels, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A source material mapped onto the engine's simple material model.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMaterial {
    pub name: String,
    /// Linear RGBA base colour factor.
    pub base_color: [f32; 4],
    /// Index into [`ImportedModel::textures`].
    pub base_texture: Option<usize>,
    pub alpha: AlphaSetting,
    pub double_sided: bool,
    pub unlit: bool,
    /// Linear RGB emissive factor (emissive textures are not supported).
    pub emissive: [f32; 3],
}

impl Default for ImportedMaterial {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            base_color: [1.0; 4],
            base_texture: None,
            alpha: AlphaSetting::Opaque,
            double_sided: false,
            unlit: false,
            emissive: [0.0; 3],
        }
    }
}

/// One drawable piece: a mesh primitive with node transforms baked in, or,
/// for a skinned part, in the skin's bind space (drawn through the
/// skeleton's joint palette).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPart {
    pub name: String,
    pub mesh: CpuMesh,
    /// Index into [`ImportedModel::materials`].
    pub material: usize,
    /// Vertices are deformed by [`ImportedModel::skeleton`].
    pub skinned: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedModel {
    pub parts: Vec<ModelPart>,
    pub materials: Vec<ImportedMaterial>,
    pub textures: Vec<CpuTexture>,
    /// The skin used by skinned parts, if any.
    pub skeleton: Option<crate::anim::Skeleton>,
    /// Animation clips for `skeleton`, with unique names.
    pub clips: Vec<crate::anim::AnimationClip>,
    /// Things that were ignored or approximated.
    pub warnings: Vec<String>,
}

impl ImportedModel {
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.mesh.triangle_count()).sum()
    }

    /// Axis-aligned bounds of all parts in the rest pose, or `None` when
    /// empty.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.parts.iter().flat_map(|p| self.rest_positions(p));
        let first = it.next()?.to_array();
        let (mut lo, mut hi) = (first, first);
        for v in it {
            let v = v.to_array();
            for i in 0..3 {
                lo[i] = lo[i].min(v[i]);
                hi[i] = hi[i].max(v[i]);
            }
        }
        Some((lo, hi))
    }

    /// Joint matrices of the rest pose (empty without a skeleton).
    pub fn rest_palette(&self) -> Vec<glam::Mat4> {
        self.skeleton
            .as_ref()
            .map(|s| s.palette(&s.rest_pose()))
            .unwrap_or_default()
    }

    /// `part`'s vertex positions in model space in the rest pose (skinned
    /// parts are deformed by the rest palette).
    pub fn rest_positions(&self, part: &ModelPart) -> Vec<glam::Vec3> {
        if part.skinned {
            crate::anim::skin_positions(
                part.mesh
                    .vertices
                    .iter()
                    .map(|v| (v.position, v.joints, v.weights)),
                &self.rest_palette(),
            )
        } else {
            part.mesh
                .vertices
                .iter()
                .map(|v| glam::Vec3::from(v.position))
                .collect()
        }
    }

    pub fn clip(&self, name: &str) -> Option<&crate::anim::AnimationClip> {
        self.clips.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is {size} bytes; the import limit is {MAX_SOURCE_BYTES}")]
    TooLarge { path: PathBuf, size: u64 },
    #[error("{path}: missing dependency {dependency}")]
    MissingDependency { path: PathBuf, dependency: PathBuf },
    #[error(
        "{path}: reference `{reference}` resolves outside the project directory; \
         keep model files and their buffers/images inside the project"
    )]
    OutsideProject { path: PathBuf, reference: String },
    #[error(
        "{path}: unsupported reference `{reference}` (only relative paths and data: URIs are read; nothing is downloaded)"
    )]
    UnsupportedUri { path: PathBuf, reference: String },
    #[error("{path} is not a valid glTF file: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{path}: required extension `{extension}` is not supported")]
    UnsupportedExtension { path: PathBuf, extension: String },
    #[error("{path}: {what}: {message}")]
    Malformed {
        path: PathBuf,
        what: String,
        message: String,
    },
    #[error("{path}: image {what} could not be decoded: {message}")]
    Image {
        path: PathBuf,
        what: String,
        message: String,
    },
    #[error("{path}: the model contains no triangles")]
    Empty { path: PathBuf },
    #[error("{path}: importer failed unexpectedly ({message}); please report this file")]
    Internal { path: PathBuf, message: String },
}

/// Resolves a relative reference found in `file` (a model) against the
/// file's directory and checks it stays within `boundary`.
pub(crate) fn resolve_reference(
    file: &Path,
    boundary: &Path,
    reference: &str,
) -> Result<PathBuf, ImportError> {
    let decoded = percent_decode(reference).ok_or_else(|| ImportError::UnsupportedUri {
        path: file.to_path_buf(),
        reference: reference.to_owned(),
    })?;
    let lower = decoded.to_ascii_lowercase();
    let has_scheme = decoded.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    });
    if has_scheme || lower.starts_with("//") || Path::new(&decoded).is_absolute() {
        return Err(ImportError::UnsupportedUri {
            path: file.to_path_buf(),
            reference: reference.to_owned(),
        });
    }
    let dir = file.parent().unwrap_or(Path::new("."));
    let candidate = dir.join(&decoded);
    if !candidate.exists() {
        return Err(ImportError::MissingDependency {
            path: file.to_path_buf(),
            dependency: candidate,
        });
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|source| ImportError::Read {
            path: candidate.clone(),
            source,
        })?;
    let root = boundary
        .canonicalize()
        .map_err(|source| ImportError::Read {
            path: boundary.to_path_buf(),
            source,
        })?;
    if !canonical.starts_with(&root) {
        return Err(ImportError::OutsideProject {
            path: file.to_path_buf(),
            reference: reference.to_owned(),
        });
    }
    Ok(canonical)
}

/// Decodes `%XX` escapes. `None` for malformed escapes or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub(crate) fn read_source(path: &Path) -> Result<Vec<u8>, ImportError> {
    let read_err = |source| ImportError::Read {
        path: path.to_path_buf(),
        source,
    };
    let size = std::fs::metadata(path).map_err(read_err)?.len();
    if size > MAX_SOURCE_BYTES {
        return Err(ImportError::TooLarge {
            path: path.to_path_buf(),
            size,
        });
    }
    std::fs::read(path).map_err(read_err)
}

/// Runs an importer, turning an unexpected panic in a third-party decoder
/// into an error so a bad file cannot take down the editor.
pub(crate) fn guarded<T>(
    path: &Path,
    f: impl FnOnce() -> Result<T, ImportError>,
) -> Result<T, ImportError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned());
            Err(ImportError::Internal {
                path: path.to_path_buf(),
                message,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b.bin").unwrap(), "a b.bin");
        assert_eq!(percent_decode("plain").unwrap(), "plain");
        assert!(percent_decode("bad%2").is_none());
        assert!(percent_decode("bad%zz").is_none());
        assert!(percent_decode("%ff").is_none());
    }

    #[test]
    fn references_must_stay_inside_the_boundary() {
        let root = std::env::temp_dir().join(format!("psx_ref_{}", std::process::id()));
        let inner = root.join("project/assets");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("a b.bin"), b"x").unwrap();
        std::fs::write(root.join("secret.bin"), b"x").unwrap();
        let model = inner.join("m.gltf");
        let boundary = root.join("project");
        assert!(resolve_reference(&model, &boundary, "a%20b.bin").is_ok());
        assert!(matches!(
            resolve_reference(&model, &boundary, "../../secret.bin"),
            Err(ImportError::OutsideProject { .. })
        ));
        assert!(matches!(
            resolve_reference(&model, &boundary, "missing.bin"),
            Err(ImportError::MissingDependency { .. })
        ));
        for uri in [
            "https://example.com/a.bin",
            "file:///etc/passwd",
            "//host/share",
        ] {
            assert!(
                matches!(
                    resolve_reference(&model, &boundary, uri),
                    Err(ImportError::UnsupportedUri { .. })
                ),
                "{uri}"
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
