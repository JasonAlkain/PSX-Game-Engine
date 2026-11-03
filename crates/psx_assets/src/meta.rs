//! Asset identity: a `<source>.meta.json` sidecar next to each source file.
//!
//! ```json
//! { "format": "psx_asset_meta", "version": 1,
//!   "id": "…uuid…", "importer": "gltf",
//!   "settings": { "scale": 1.0, "max_texture_size": 256 } }
//! ```
//!
//! The ID lives in the sidecar, not in the source path, so moving or
//! renaming a source together with its sidecar keeps every scene reference.

use std::path::{Path, PathBuf};

use psx_core::AssetId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const META_FORMAT: &str = "psx_asset_meta";
pub const META_VERSION: u32 = 1;
pub const META_SUFFIX: &str = ".meta.json";

/// Which importer reads the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    /// Static glTF 2.0 model (`.gltf` or `.glb`).
    Gltf,
    /// PNG or JPEG image.
    Texture,
}

impl AssetKind {
    /// Importer for a source file, chosen by extension.
    pub fn for_path(path: &Path) -> Option<AssetKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "gltf" | "glb" => Some(AssetKind::Gltf),
            "png" | "jpg" | "jpeg" => Some(AssetKind::Texture),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Gltf => "model",
            AssetKind::Texture => "texture",
        }
    }
}

/// Importer options. Part of the import result's identity: changing them
/// changes the imported data.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ImportSettings {
    /// Uniform scale applied to model positions (glTF units are meters, so
    /// usually 1). Ignored for textures.
    pub scale: f32,
    /// Textures larger than this on either side are reduced (nearest
    /// neighbour, aspect kept) at import. An authoring budget, not a claim
    /// about original hardware.
    pub max_texture_size: u32,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            max_texture_size: 1024,
        }
    }
}

impl ImportSettings {
    pub fn problem(&self) -> Option<String> {
        if !self.scale.is_finite() || self.scale.abs() < 1e-6 {
            return Some(format!("scale {} must be finite and non-zero", self.scale));
        }
        if !(1..=8192).contains(&self.max_texture_size) {
            return Some(format!(
                "max_texture_size {} is outside 1..=8192",
                self.max_texture_size
            ));
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetMeta {
    pub format: String,
    pub version: u32,
    pub id: AssetId,
    pub importer: AssetKind,
    #[serde(default)]
    pub settings: ImportSettings,
}

impl AssetMeta {
    pub fn new(importer: AssetKind) -> Self {
        Self {
            format: META_FORMAT.to_owned(),
            version: META_VERSION,
            id: AssetId::new(),
            importer,
            settings: ImportSettings::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum MetaError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid asset metadata: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path}: {reason}")]
    Invalid { path: PathBuf, reason: String },
}

/// `model.glb` → `model.glb.meta.json`.
pub fn meta_path(source: &Path) -> PathBuf {
    let mut name = source.file_name().unwrap_or_default().to_os_string();
    name.push(META_SUFFIX);
    source.with_file_name(name)
}

/// The source path a sidecar belongs to, if `path` is a sidecar.
pub fn source_of_meta(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let source = name.strip_suffix(META_SUFFIX)?;
    (!source.is_empty()).then(|| path.with_file_name(source))
}

pub fn parse_meta(text: &str, path: &Path) -> Result<AssetMeta, MetaError> {
    let meta: AssetMeta = serde_json::from_str(text).map_err(|source| MetaError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    let invalid = |reason: String| MetaError::Invalid {
        path: path.to_path_buf(),
        reason,
    };
    if meta.format != META_FORMAT {
        return Err(invalid(format!("expected \"format\": \"{META_FORMAT}\"")));
    }
    if meta.version == 0 || meta.version > META_VERSION {
        return Err(invalid(format!(
            "version {} is not supported (this build reads up to {META_VERSION})",
            meta.version
        )));
    }
    if let Some(p) = meta.settings.problem() {
        return Err(invalid(p));
    }
    Ok(meta)
}

pub fn load_meta(path: &Path) -> Result<AssetMeta, MetaError> {
    let text = std::fs::read_to_string(path).map_err(|source| MetaError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_meta(&text, path)
}

pub fn save_meta(meta: &AssetMeta, path: &Path) -> Result<(), MetaError> {
    let mut text = serde_json::to_string_pretty(meta).map_err(|source| MetaError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    psx_core::fsutil::atomic_write(path, text.as_bytes()).map_err(|source| MetaError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_kinds() {
        let src = Path::new("assets/props/crate.GLB");
        assert_eq!(AssetKind::for_path(src), Some(AssetKind::Gltf));
        assert_eq!(
            AssetKind::for_path(Path::new("a.jpeg")),
            Some(AssetKind::Texture)
        );
        assert_eq!(AssetKind::for_path(Path::new("a.fbx")), None);
        let meta = meta_path(src);
        assert_eq!(meta, Path::new("assets/props/crate.GLB.meta.json"));
        assert_eq!(source_of_meta(&meta).unwrap(), src);
        assert_eq!(source_of_meta(Path::new(".meta.json")), None);
    }

    #[test]
    fn meta_round_trip_and_rejections() {
        let meta = AssetMeta::new(AssetKind::Gltf);
        let text = serde_json::to_string(&meta).unwrap();
        assert_eq!(parse_meta(&text, Path::new("m")).unwrap(), meta);
        let future = text.replace("\"version\":1", "\"version\":2");
        assert!(parse_meta(&future, Path::new("m")).is_err());
        let bad_scale = text.replace("\"scale\":1.0", "\"scale\":0.0");
        assert!(
            parse_meta(&bad_scale, Path::new("m"))
                .unwrap_err()
                .to_string()
                .contains("scale")
        );
    }
}
