//! Versioned scene files.
//!
//! ```json
//! { "format": "psx_scene", "version": 1, "scene": { ... } }
//! ```
//!
//! Loading checks the format tag, rejects versions newer than this build
//! (and version 0), rejects unknown fields, and validates the hierarchy and
//! values ([`Scene::problems`]). Saving validates first and writes
//! atomically, keeping the previous file as `<name>.bak`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::scene::{Scene, SceneProblem};

pub const SCENE_FORMAT: &str = "psx_scene";
/// Current scene file version. Older versions would be migrated in
/// [`parse_scene`]; newer versions are rejected.
pub const SCENE_VERSION: u32 = 1;
/// Conventional file extension.
pub const SCENE_EXTENSION: &str = "scene.json";

#[derive(Debug, Error)]
pub enum SceneFileError {
    #[error("could not read scene file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write scene file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("scene file {path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path} is not a scene file (expected \"format\": \"{SCENE_FORMAT}\" and a version)")]
    WrongFormat { path: PathBuf },
    #[error(
        "scene file {path} has version {found}; this build supports up to {SCENE_VERSION}. \
         Update the engine to open it."
    )]
    UnsupportedVersion { path: PathBuf, found: u64 },
    #[error("scene file {path} has invalid contents: {source}")]
    Schema {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("scene {path} is invalid:\n{}", list(.problems))]
    Invalid {
        path: PathBuf,
        problems: Vec<SceneProblem>,
    },
}

fn list(problems: &[SceneProblem]) -> String {
    problems
        .iter()
        .map(|p| format!("  - {p}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Serialize)]
struct SceneFileOut<'a> {
    format: &'static str,
    version: u32,
    scene: &'a Scene,
}

/// Parses scene JSON. `path` is used only in error messages.
pub fn parse_scene(text: &str, path: &Path) -> Result<Scene, SceneFileError> {
    let path_buf = || path.to_path_buf();
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|source| SceneFileError::Json {
            path: path_buf(),
            source,
        })?;
    if value.get("format").and_then(|f| f.as_str()) != Some(SCENE_FORMAT) {
        return Err(SceneFileError::WrongFormat { path: path_buf() });
    }
    let version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or(SceneFileError::WrongFormat { path: path_buf() })?;
    if version == 0 || version > u64::from(SCENE_VERSION) {
        return Err(SceneFileError::UnsupportedVersion {
            path: path_buf(),
            found: version,
        });
    }
    if let Some(extra) = value.as_object().and_then(|o| {
        o.keys()
            .find(|k| !["format", "version", "scene"].contains(&k.as_str()))
    }) {
        return Err(SceneFileError::Schema {
            path: path_buf(),
            source: serde::de::Error::custom(format!("unknown top-level field `{extra}`")),
        });
    }
    // Version 1 is current; migrations from older versions go here.
    let scene_value = value
        .get("scene")
        .cloned()
        .ok_or(SceneFileError::WrongFormat { path: path_buf() })?;
    let scene: Scene =
        serde_json::from_value(scene_value).map_err(|source| SceneFileError::Schema {
            path: path_buf(),
            source,
        })?;
    let problems = scene.problems();
    if !problems.is_empty() {
        return Err(SceneFileError::Invalid {
            path: path_buf(),
            problems,
        });
    }
    Ok(scene)
}

/// Serialises a valid scene to pretty JSON with the format header.
pub fn scene_to_json(scene: &Scene, path: &Path) -> Result<String, SceneFileError> {
    let problems = scene.problems();
    if !problems.is_empty() {
        return Err(SceneFileError::Invalid {
            path: path.to_path_buf(),
            problems,
        });
    }
    let out = SceneFileOut {
        format: SCENE_FORMAT,
        version: SCENE_VERSION,
        scene,
    };
    // Plain data with finite numbers (checked above) always serialises.
    let mut text = serde_json::to_string_pretty(&out).map_err(|source| SceneFileError::Schema {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    Ok(text)
}

pub fn load_scene(path: &Path) -> Result<Scene, SceneFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SceneFileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_scene(&text, path)
}

/// Validates and writes atomically (previous file kept as `<name>.bak`).
/// The caller decides the revision; see [`crate::Scene::revision`].
pub fn save_scene(scene: &Scene, path: &Path) -> Result<(), SceneFileError> {
    let text = scene_to_json(scene, path)?;
    psx_core::fsutil::atomic_write(path, text.as_bytes()).map_err(|source| SceneFileError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::*;
    use crate::transform::euler_deg_to_quat;
    use glam::Vec3;
    use psx_core::AssetId;

    fn sample() -> Scene {
        let mut scene = Scene::new("Room");
        let mut floor = Entity::new("Floor");
        floor.transform.scale = Vec3::new(8.0, 1.0, 8.0);
        floor.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Plane,
                texture: Some(AssetId::new()),
            },
            material: MaterialOverride::default(),
        });
        let mut crate_ = Entity::new("Crate");
        crate_.parent = Some(floor.id);
        crate_.transform.rotation = euler_deg_to_quat(Vec3::new(0.0, 30.0, 0.0));
        crate_.mesh = Some(MeshComponent {
            source: MeshSource::Model {
                asset: AssetId::new(),
            },
            material: MaterialOverride {
                tint: Some([1.0, 0.5, 0.5, 1.0]),
                alpha: Some(AlphaSetting::Cutout { threshold: 0.4 }),
                double_sided: Some(true),
                ..Default::default()
            },
        });
        scene.entities = vec![floor, crate_];
        scene
    }

    #[test]
    fn round_trip_is_exact() {
        let scene = sample();
        let p = Path::new("room.scene.json");
        let text = scene_to_json(&scene, p).unwrap();
        assert!(text.contains("\"format\": \"psx_scene\""));
        assert_eq!(parse_scene(&text, p).unwrap(), scene);
        // Serialising again gives identical text (stable diffs).
        assert_eq!(
            scene_to_json(&parse_scene(&text, p).unwrap(), p).unwrap(),
            text
        );
    }

    #[test]
    fn rejects_future_versions_wrong_format_and_unknown_fields() {
        let p = Path::new("x");
        let text = scene_to_json(&sample(), p).unwrap();
        let future = text.replace("\"version\": 1", "\"version\": 99");
        assert!(matches!(
            parse_scene(&future, p),
            Err(SceneFileError::UnsupportedVersion { found: 99, .. })
        ));
        let zero = text.replace("\"version\": 1", "\"version\": 0");
        assert!(matches!(
            parse_scene(&zero, p),
            Err(SceneFileError::UnsupportedVersion { found: 0, .. })
        ));
        assert!(matches!(
            parse_scene("{\"format\":\"psx_style\",\"version\":1}", p),
            Err(SceneFileError::WrongFormat { .. })
        ));
        let unknown = text.replace("\"name\": \"Crate\"", "\"name\": \"Crate\", \"colour\": 1");
        assert!(matches!(
            parse_scene(&unknown, p),
            Err(SceneFileError::Schema { .. })
        ));
        assert!(matches!(
            parse_scene("{", p),
            Err(SceneFileError::Json { .. })
        ));
    }

    #[test]
    fn invalid_scenes_are_neither_loaded_nor_saved() {
        let mut scene = sample();
        let floor = scene.entities[0].id;
        scene.entities[0].parent = Some(scene.entities[1].id);
        scene.entities[1].parent = Some(floor);
        let p = Path::new("x");
        let err = scene_to_json(&scene, p).unwrap_err();
        assert!(err.to_string().contains("cycle"), "{err}");
    }

    #[test]
    fn save_and_load_from_disk() {
        let dir = std::env::temp_dir().join(format!("psx_scene_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.scene.json");
        let scene = sample();
        save_scene(&scene, &path).unwrap();
        assert_eq!(load_scene(&path).unwrap(), scene);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
