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
/// Current scene file version. Older versions are migrated in
/// [`parse_scene`]; newer versions are rejected.
///
/// - 1: entities with transform, mesh and material override.
/// - 2: adds the optional `collider`, `audio` and `behaviors` entity
///   fields. A version 1 file is a valid version 2 file without them, so
///   the migration only changes the header on the next save.
/// - 3: adds the optional `light`, `animator` and `shadow` entity fields,
///   and the style's `textures`, `shading` and `shadows` sections (style
///   version 2; missing sections get the PS1 values). Older files load
///   unchanged.
/// - 4: adds the optional `tile_size` on primitive mesh sources. Older files
///   load unchanged and keep their textures covering each face once.
pub const SCENE_VERSION: u32 = 4;
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
    // Migrations from older versions go here. 1 -> 2 -> 3 -> 4 only added
    // optional fields (with defaults), so older documents deserialise
    // unchanged.
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
    copperframe_core::fsutil::atomic_write(path, text.as_bytes()).map_err(|source| {
        SceneFileError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::*;
    use crate::transform::euler_deg_to_quat;
    use copperframe_core::AssetId;
    use glam::Vec3;

    fn sample() -> Scene {
        let mut scene = Scene::new("Room");
        let mut floor = Entity::new("Floor");
        floor.transform.scale = Vec3::new(8.0, 1.0, 8.0);
        floor.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Plane,
                texture: Some(AssetId::new()),
                tile_size: None,
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
        let future = text.replace("\"version\": 4", "\"version\": 99");
        assert!(matches!(
            parse_scene(&future, p),
            Err(SceneFileError::UnsupportedVersion { found: 99, .. })
        ));
        let zero = text.replace("\"version\": 4", "\"version\": 0");
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
    fn version_1_files_load_and_save_as_current_version() {
        let p = Path::new("old.scene.json");
        // The example room as committed with scene version 1 (Milestone 3).
        let v1 = include_str!("../fixtures/v1_room.scene.json");
        assert!(v1.contains("\"version\": 1"));
        let scene = match parse_scene(v1, p) {
            Ok(scene) => scene,
            Err(err) => panic!("version 1 fixture no longer loads: {err}"),
        };
        assert_eq!(scene.revision, 1);
        assert_eq!(scene.entities.len(), 14);
        assert!(scene.entities[0].collider.is_none());
        assert!(scene.entities[0].behaviors.is_empty());
        let text = scene_to_json(&scene, p).unwrap();
        assert!(text.contains("\"version\": 4"));
    }

    #[test]
    fn version_2_files_load_with_ps1_values_for_new_style_sections() {
        let p = Path::new("v2.scene.json");
        // The Lever Room as committed with scene version 2 (Milestone 4).
        let v2 = include_str!("../fixtures/v2_room.scene.json");
        assert!(v2.contains("\"version\": 2") && !v2.contains("\"textures\""));
        let scene = match parse_scene(v2, p) {
            Ok(scene) => scene,
            Err(err) => panic!("version 2 fixture no longer loads: {err}"),
        };
        assert_eq!(scene.settings.style, copperframe_core::RetroStyle::ps1());
        assert!(scene.entities.iter().any(|e| !e.behaviors.is_empty()));
        assert!(scene.entities.iter().all(|e| e.light.is_none()));
        let text = scene_to_json(&scene, p).unwrap();
        assert!(text.contains("\"version\": 4"));
        assert_eq!(parse_scene(&text, p).unwrap(), scene);
    }

    #[test]
    fn version_3_files_load_without_tiling() {
        let p = Path::new("v3.scene.json");
        // The Lever Room as committed with scene version 3 (Milestone 5).
        let v3 = include_str!("../fixtures/v3_room.scene.json");
        assert!(v3.contains("\"version\": 3") && !v3.contains("tile_size"));
        let scene = match parse_scene(v3, p) {
            Ok(scene) => scene,
            Err(err) => panic!("version 3 fixture no longer loads: {err}"),
        };
        let text = scene_to_json(&scene, p).unwrap();
        assert!(text.contains("\"version\": 4") && !text.contains("tile_size"));
        assert_eq!(parse_scene(&text, p).unwrap(), scene);
    }

    #[test]
    fn tile_sizes_round_trip_and_bad_ones_are_rejected() {
        let p = Path::new("tile.scene.json");
        let mut scene = sample();
        let set = |scene: &mut Scene, size: f32| {
            scene.entities[0].mesh = Some(MeshComponent {
                source: MeshSource::Primitive {
                    shape: PrimitiveShape::Plane,
                    texture: None,
                    tile_size: Some(size),
                },
                material: Default::default(),
            });
        };
        set(&mut scene, 2.5);
        let text = scene_to_json(&scene, p).unwrap();
        assert!(text.contains("\"tile_size\": 2.5"));
        assert_eq!(parse_scene(&text, p).unwrap(), scene);
        for bad in [0.0, -1.0, f32::NAN, 1.0e6] {
            set(&mut scene, bad);
            assert!(!scene.problems().is_empty(), "{bad}");
        }
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
