//! Project layout, asset database, editor lock and validation.
//!
//! ```text
//! my_game/
//!   project.json          {"format": "psx_project", "version": 4, ...}
//!   input.json            default key bindings (optional, see [`crate::input`])
//!   assets/               source files + <source>.meta.json sidecars
//!   scenes/               *.scene.json
//!   .psx/editor.lock      present while an editor has the project open
//! ```
//!
//! Paths stored in project files are relative to the project root and use
//! `/` separators, so projects move between machines and operating systems.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use copperframe_core::{AssetId, EntityId};
use copperframe_scene::{
    BehaviorComponent, ColliderComponent, ColliderShape, Entity, MeshComponent, MeshSource,
    PrimitiveShape, RigidbodyComponent, Scene,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::import::{CpuTexture, ImportError, ImportedModel};
use crate::meta::{self, AssetKind, AssetMeta, ImportSettings, MetaError};
pub use crate::project_settings::{
    AudioSettings, BuildSettings, BusSettings, DisplaySettings, GraphicsStyle, MAX_GRAVITY,
    PhysicsSettings, ScriptingSettings, StyleSettings, file_slug,
};

pub const PROJECT_FILE: &str = "project.json";
pub const PROJECT_FORMAT: &str = "psx_project";
/// Version 2 added the `display`, `style`, `build` and `physics` sections;
/// 3 `style.look`; 4 `audio`; 5 `scripting`; 6 `scripting.csharp`.
pub const PROJECT_VERSION: u32 = 6;
pub const ASSETS_DIR: &str = "assets";
pub const SCENES_DIR: &str = "scenes";
pub const LOCK_FILE: &str = ".psx/editor.lock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// Scene opened first, relative to the project root.
    pub startup_scene: String,
    #[serde(default)]
    pub display: DisplaySettings,
    #[serde(default)]
    pub style: StyleSettings,
    #[serde(default)]
    pub build: BuildSettings,
    #[serde(default)]
    pub physics: PhysicsSettings,
    /// Since version 4.
    #[serde(default)]
    pub audio: AudioSettings,
    /// Since version 5. Older files get the default (Lua on), which
    /// changes nothing for a project without `.lua` files.
    #[serde(default)]
    pub scripting: ScriptingSettings,
}

impl ProjectFile {
    /// Checks the values a hand-edited file could get wrong. The editor's
    /// Project Settings panel runs the same check before saving.
    pub fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("the project name is empty".into());
        }
        if !is_safe_relative(&self.startup_scene) {
            return Err(format!(
                "startup_scene `{}` must be a relative path inside the project",
                self.startup_scene
            ));
        }
        let d = &self.display;
        if !DisplaySettings::WIDTH_RANGE.contains(&d.width)
            || !DisplaySettings::HEIGHT_RANGE.contains(&d.height)
        {
            return Err(format!(
                "display size {}x{} is outside {}..={} x {}..={}",
                d.width,
                d.height,
                DisplaySettings::WIDTH_RANGE.start(),
                DisplaySettings::WIDTH_RANGE.end(),
                DisplaySettings::HEIGHT_RANGE.start(),
                DisplaySettings::HEIGHT_RANGE.end()
            ));
        }
        if let Some(p) = self.physics.problem() {
            return Err(p);
        }
        if let Some(p) = self.audio.problem() {
            return Err(p);
        }
        self.style
            .look
            .validate()
            .map_err(|e| format!("style.look: {e}"))?;
        let script = self.build.check_script.trim();
        if !script.is_empty() && !is_safe_relative(script) {
            return Err(format!(
                "build check_script `{script}` must be a relative path inside the project"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0} already contains a project")]
    AlreadyExists(PathBuf),
    #[error("no project at {0} (missing {PROJECT_FILE}); create one with `copperframe new`")]
    NotAProject(PathBuf),
    #[error("{path} is not a valid project file: {reason}")]
    Invalid { path: PathBuf, reason: String },
    #[error(transparent)]
    Scene(#[from] copperframe_scene::SceneFileError),
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error("{0} is not a supported asset type (glTF/GLB, PNG, JPEG)")]
    UnsupportedAsset(PathBuf),
    #[error("{0} is outside the project's assets folder")]
    OutsideAssets(PathBuf),
    #[error(
        "the project is open in a running editor (lock file {path}, pid {pid}). Close that \
         editor first"
    )]
    Locked { path: PathBuf, pid: u32 },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub file: ProjectFile,
}

impl Project {
    /// Creates a new project whose starter scene is playable: a floor, a
    /// spinning cube and a first-person player (engine behaviours only).
    pub fn create(root: &Path, name: &str) -> Result<Project, ProjectError> {
        if root.join(PROJECT_FILE).exists() {
            return Err(ProjectError::AlreadyExists(root.to_path_buf()));
        }
        for dir in [
            root.to_path_buf(),
            root.join(ASSETS_DIR),
            root.join(SCENES_DIR),
        ] {
            std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        }
        let file = ProjectFile {
            format: PROJECT_FORMAT.to_owned(),
            version: PROJECT_VERSION,
            name: name.to_owned(),
            startup_scene: format!("{SCENES_DIR}/main.scene.json"),
            display: DisplaySettings::default(),
            style: StyleSettings::default(),
            build: BuildSettings::default(),
            physics: PhysicsSettings::default(),
            audio: AudioSettings::default(),
            scripting: ScriptingSettings::default(),
        };
        let project = Project {
            root: root.to_path_buf(),
            file,
        };
        let mut scene = Scene::new("Main");
        scene.settings.look = copperframe_scene::LookSource::Project;
        let mut floor = Entity::new("Floor");
        floor.transform.scale = glam::Vec3::new(8.0, 1.0, 8.0);
        floor.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Plane,
                texture: None,
                tile_size: Some(2.0),
            },
            material: Default::default(),
        });
        floor.collider = Some(ColliderComponent {
            offset: glam::Vec3::new(0.0, -0.1, 0.0),
            ..ColliderComponent::new(ColliderShape::Box {
                size: glam::Vec3::new(1.0, 0.2, 1.0),
            })
        });
        let mut cube = Entity::new("Cube");
        cube.transform.translation = glam::Vec3::new(0.0, 0.5, -1.5);
        cube.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Cube,
                texture: None,
                tile_size: Some(copperframe_scene::DEFAULT_TILE_SIZE),
            },
            material: Default::default(),
        });
        cube.collider = Some(ColliderComponent::new(ColliderShape::Box {
            size: glam::Vec3::ONE,
        }));
        cube.rigidbody = Some(RigidbodyComponent::kinematic());
        cube.behaviors = vec![BehaviorComponent::new("psx.spin")];
        // A physics crate that drops onto the floor and can be pushed.
        let mut crate_ = Entity::new("Crate");
        crate_.transform.translation = glam::Vec3::new(1.5, 2.0, -0.5);
        crate_.transform.scale = glam::Vec3::splat(0.6);
        crate_.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Cube,
                texture: None,
                tile_size: Some(copperframe_scene::DEFAULT_TILE_SIZE),
            },
            material: Default::default(),
        });
        crate_.collider = Some(ColliderComponent::new(ColliderShape::Box {
            size: glam::Vec3::ONE,
        }));
        crate_.rigidbody = Some(RigidbodyComponent::dynamic());
        let mut player = Entity::new("Player");
        player.transform.translation = glam::Vec3::new(0.0, 0.0, 2.5);
        player.behaviors = vec![BehaviorComponent::new("psx.player")];
        scene.entities.extend([floor, cube, crate_, player]);
        copperframe_scene::save_scene(&scene, &project.startup_scene_path())?;
        project.save()?;
        let ignore = root.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, ".psx/\ndist/\n*.tmp\n*.bak\n").map_err(io_err(&ignore))?;
        }
        Ok(project)
    }

    pub fn open(root: &Path) -> Result<Project, ProjectError> {
        let path = root.join(PROJECT_FILE);
        if !path.exists() {
            return Err(ProjectError::NotAProject(root.to_path_buf()));
        }
        let text = std::fs::read_to_string(&path).map_err(io_err(&path))?;
        let invalid = |reason: String| ProjectError::Invalid {
            path: path.clone(),
            reason,
        };
        let file: ProjectFile = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
        if file.format != PROJECT_FORMAT {
            return Err(invalid(format!(
                "expected \"format\": \"{PROJECT_FORMAT}\""
            )));
        }
        if file.version == 0 || file.version > PROJECT_VERSION {
            return Err(invalid(format!(
                "version {} is not supported (this build reads up to {PROJECT_VERSION})",
                file.version
            )));
        }
        file.check().map_err(invalid)?;
        Ok(Project {
            root: root.to_path_buf(),
            file,
        })
    }

    /// Writes `project.json` in the current version (an older file is
    /// upgraded the first time it is saved).
    pub fn save(&self) -> Result<(), ProjectError> {
        let path = self.root.join(PROJECT_FILE);
        let file = ProjectFile {
            version: PROJECT_VERSION,
            ..self.file.clone()
        };
        let mut text = serde_json::to_string_pretty(&file).map_err(|e| ProjectError::Invalid {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        text.push('\n');
        copperframe_core::fsutil::atomic_write(&path, text.as_bytes()).map_err(io_err(&path))
    }

    /// Where Build > Export Game writes by default: the build settings'
    /// folder (relative to the project, or absolute), else
    /// `dist/<project name>`.
    pub fn export_dir(&self) -> PathBuf {
        let dir = self.file.build.export_dir.trim();
        if dir.is_empty() {
            self.root.join("dist").join(file_slug(&self.file.name))
        } else {
            self.root.join(dir)
        }
    }

    /// The test script the post-export check runs, if one is set.
    pub fn export_check_script(&self) -> Option<PathBuf> {
        let s = self.file.build.check_script.trim();
        (!s.is_empty()).then(|| self.resolve(s))
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join(ASSETS_DIR)
    }

    pub fn resolve(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn startup_scene_path(&self) -> PathBuf {
        self.resolve(&self.file.startup_scene)
    }

    /// `path` relative to the project root with `/` separators.
    pub fn relative(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Every `*.scene.json` under `scenes/`, sorted.
    pub fn scene_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk(&self.root.join(SCENES_DIR), &mut |p| {
            if p.to_string_lossy().ends_with(".scene.json") {
                out.push(p.to_path_buf());
            }
        });
        out.sort();
        out
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// The pid of the running editor that has the project open, if any
    /// (0 when the pid cannot be read). A lock file left behind by an
    /// editor that crashed or was killed is ignored: the editor holds an OS
    /// file lock on it, which the OS releases when the process ends.
    pub fn editor_lock(&self) -> Option<u32> {
        let path = self.lock_path();
        let file = std::fs::File::open(&path).ok()?;
        match file.try_lock_shared() {
            Ok(()) => {
                // Nobody holds it: a stale lock from a dead editor.
                let _ = file.unlock();
                None
            }
            Err(std::fs::TryLockError::WouldBlock) => Some(read_pid(&path)),
            // The file system has no locks (some network shares): trust
            // the file, as before.
            Err(std::fs::TryLockError::Error(_)) => Some(read_pid(&path)),
        }
    }

    /// Fails with [`ProjectError::Locked`] while a running editor has the
    /// project open.
    pub fn check_unlocked(&self) -> Result<(), ProjectError> {
        match self.editor_lock() {
            Some(pid) => Err(ProjectError::Locked {
                path: self.lock_path(),
                pid,
            }),
            None => Ok(()),
        }
    }
}

fn is_safe_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && !path.is_absolute()
        && !p.contains(':')
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Visits files under `dir` (skipping hidden entries and symlinked folders).
fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(ty) = entry.file_type() else { continue };
        let path = entry.path();
        if ty.is_dir() {
            walk(&path, f);
        } else if ty.is_file() {
            f(&path);
        }
    }
}

/// The pid written in a lock file (0 if unreadable; Windows refuses reads
/// of a file another process has locked).
fn read_pid(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Held by the editor while a project is open: the lock file records the
/// editor's pid and stays exclusively locked (an OS file lock) until the
/// editor closes or dies. The CLI writes scene files only when no running
/// editor holds it.
#[derive(Debug)]
pub struct EditorLock {
    path: PathBuf,
    /// Holds the OS lock; closed before the file is removed.
    file: Option<std::fs::File>,
}

impl EditorLock {
    /// Takes the lock. A lock file left by an editor that is no longer
    /// running is replaced automatically. With `steal`, the lock file of a
    /// running editor is replaced too (both editors then think they own the
    /// project; only for recovering from a hung editor).
    pub fn acquire(project: &Project, steal: bool) -> Result<EditorLock, ProjectError> {
        let path = project.lock_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let open = || {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
        };
        let mut file = open().map_err(io_err(&path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) if steal => {
                drop(file);
                std::fs::remove_file(&path).map_err(io_err(&path))?;
                file = open().map_err(io_err(&path))?;
                file.try_lock().map_err(|e| ProjectError::Io {
                    path: path.clone(),
                    source: e.into(),
                })?;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(ProjectError::Locked {
                    path: path.clone(),
                    pid: read_pid(&path),
                });
            }
            Err(std::fs::TryLockError::Error(_)) => {
                // No OS locks on this file system: fall back to the file's
                // presence, as before.
                if !steal && std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
                    return Err(ProjectError::Locked {
                        path: path.clone(),
                        pid: read_pid(&path),
                    });
                }
            }
        }
        let pid = std::process::id().to_string();
        file.set_len(0)
            .and_then(|()| std::io::Write::write_all(&mut file, pid.as_bytes()))
            .and_then(|()| file.sync_data())
            .map_err(io_err(&path))?;
        Ok(EditorLock {
            path,
            file: Some(file),
        })
    }
}

impl Drop for EditorLock {
    fn drop(&mut self) {
        // Close (and unlock) first: Windows cannot delete an open file.
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One registered asset.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetEntry {
    pub id: AssetId,
    pub kind: AssetKind,
    /// Absolute source path.
    pub source: PathBuf,
    /// Source path relative to the project root.
    pub relative: String,
    pub settings: ImportSettings,
}

/// A decoded asset.
#[derive(Debug, Clone)]
pub enum ImportedAsset {
    Model(ImportedModel),
    Texture(CpuTexture),
    Audio(crate::ImportedAudio),
}

impl ImportedAsset {
    pub fn warnings(&self) -> &[String] {
        match self {
            ImportedAsset::Model(m) => &m.warnings,
            ImportedAsset::Texture(_) | ImportedAsset::Audio(_) => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Project-relative file the problem is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetId>,
    pub message: String,
}

impl Diagnostic {
    fn new(severity: Severity, file: Option<String>, message: impl Into<String>) -> Self {
        Self {
            severity,
            file,
            entity: None,
            asset: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev}")?;
        if let Some(file) = &self.file {
            write!(f, " [{file}]")?;
        }
        if let Some(e) = self.entity {
            write!(f, " entity {e}")?;
        }
        if let Some(a) = self.asset {
            write!(f, " asset {a}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// Every registered asset in `assets/`, found through its sidecar.
#[derive(Debug, Clone, Default)]
pub struct AssetDatabase {
    pub entries: Vec<AssetEntry>,
    /// Problems found while scanning (bad sidecars, duplicates, orphans).
    pub diagnostics: Vec<Diagnostic>,
}

impl AssetDatabase {
    pub fn scan(project: &Project) -> AssetDatabase {
        let mut db = AssetDatabase::default();
        let mut by_id: HashMap<AssetId, String> = HashMap::new();
        let mut files = Vec::new();
        walk(&project.assets_dir(), &mut |p| files.push(p.to_path_buf()));
        for path in &files {
            let rel = project.relative(path);
            if let Some(source) = meta::source_of_meta(path) {
                let meta = match meta::load_meta(path) {
                    Ok(m) => m,
                    Err(e) => {
                        db.diagnostics.push(Diagnostic::new(
                            Severity::Error,
                            Some(rel),
                            e.to_string(),
                        ));
                        continue;
                    }
                };
                let source_rel = project.relative(&source);
                if !source.is_file() {
                    let mut d = Diagnostic::new(
                        Severity::Error,
                        Some(rel),
                        format!("metadata for missing source file {source_rel}"),
                    );
                    d.asset = Some(meta.id);
                    db.diagnostics.push(d);
                    continue;
                }
                if AssetKind::for_path(&source) != Some(meta.importer) {
                    let mut d = Diagnostic::new(
                        Severity::Error,
                        Some(rel),
                        format!(
                            "importer `{}` does not match the file type of {source_rel}",
                            meta.importer.label()
                        ),
                    );
                    d.asset = Some(meta.id);
                    db.diagnostics.push(d);
                    continue;
                }
                if let Some(first) = by_id.get(&meta.id) {
                    let mut d = Diagnostic::new(
                        Severity::Error,
                        Some(rel),
                        format!(
                            "asset id {} is also used by {first}; give one of them a new id \
                             (delete its .meta.json and re-register it)",
                            meta.id
                        ),
                    );
                    d.asset = Some(meta.id);
                    db.diagnostics.push(d);
                    continue;
                }
                by_id.insert(meta.id, source_rel.clone());
                db.entries.push(AssetEntry {
                    id: meta.id,
                    kind: meta.importer,
                    source,
                    relative: source_rel,
                    settings: meta.settings,
                });
            } else if AssetKind::for_path(path).is_some() && !meta::meta_path(path).exists() {
                db.diagnostics.push(Diagnostic::new(
                    Severity::Warning,
                    Some(rel),
                    "not registered (no .meta.json); scenes cannot use it until it is \
                     registered with `copperframe import` or the editor's Rescan",
                ));
            }
        }
        db
    }

    pub fn get(&self, id: AssetId) -> Option<&AssetEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Registers a source file under `assets/`: writes a sidecar with a new
    /// ID if it has none, and returns the (new or existing) metadata.
    pub fn register(project: &Project, source: &Path) -> Result<AssetMeta, ProjectError> {
        let kind = AssetKind::for_path(source)
            .ok_or_else(|| ProjectError::UnsupportedAsset(source.to_path_buf()))?;
        let canonical_assets = project
            .assets_dir()
            .canonicalize()
            .map_err(io_err(&project.assets_dir()))?;
        let canonical = source.canonicalize().map_err(io_err(source))?;
        if !canonical.starts_with(&canonical_assets) {
            return Err(ProjectError::OutsideAssets(source.to_path_buf()));
        }
        let meta_path = meta::meta_path(source);
        if meta_path.exists() {
            return Ok(meta::load_meta(&meta_path)?);
        }
        let meta = AssetMeta::new(kind);
        meta::save_meta(&meta, &meta_path)?;
        Ok(meta)
    }

    /// Registers every unregistered importable file. Returns the new paths.
    pub fn register_all(project: &Project) -> Result<Vec<PathBuf>, ProjectError> {
        let mut files = Vec::new();
        walk(&project.assets_dir(), &mut |p| {
            if AssetKind::for_path(p).is_some() && !meta::meta_path(p).exists() {
                files.push(p.to_path_buf());
            }
        });
        for f in &files {
            Self::register(project, f)?;
        }
        Ok(files)
    }

    /// Runs the importer for `entry`. External model references must stay
    /// inside the project.
    pub fn import(project: &Project, entry: &AssetEntry) -> Result<ImportedAsset, ImportError> {
        match entry.kind {
            AssetKind::Gltf => crate::import_gltf(&entry.source, &project.root, &entry.settings)
                .map(ImportedAsset::Model),
            AssetKind::Texture => {
                crate::import_texture(&entry.source, &entry.settings).map(ImportedAsset::Texture)
            }
            AssetKind::Audio => {
                crate::import_audio(&entry.source, &entry.settings).map(ImportedAsset::Audio)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub id: AssetId,
    pub path: String,
    pub kind: AssetKind,
    pub imported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triangles: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_size: Option<(u32, u32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    /// Skinned models: joint count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joints: Option<usize>,
    /// Skinned models: animation clip names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneSummary {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub project: String,
    pub errors: usize,
    pub warnings: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub assets: Vec<AssetSummary>,
    pub scenes: Vec<SceneSummary>,
}

impl ValidationReport {
    pub fn ok(&self) -> bool {
        self.errors == 0
    }
}

/// Checks a scene's asset references against the database. `failed` lists
/// assets that exist but did not import (a warning: a fallback is drawn).
pub fn check_scene_refs(
    scene: &Scene,
    db: &AssetDatabase,
    failed: &[AssetId],
    file: Option<String>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let refs = scene.entities.iter().flat_map(|e| {
        let mesh = e.mesh.as_ref().and_then(|m| match m.source {
            MeshSource::Model { asset } => Some((asset, AssetKind::Gltf)),
            MeshSource::Primitive {
                texture: Some(asset),
                ..
            } => Some((asset, AssetKind::Texture)),
            MeshSource::Primitive { texture: None, .. } => None,
        });
        let audio = e.audio.as_ref().map(|a| (a.sound, AssetKind::Audio));
        mesh.into_iter().chain(audio).map(move |r| (e, r))
    });
    for (e, (asset, want)) in refs {
        let mut d = |severity, message: String| {
            let mut d = Diagnostic::new(severity, file.clone(), message);
            d.entity = Some(e.id);
            d.asset = Some(asset);
            out.push(d);
        };
        match db.get(asset) {
            None => d(
                Severity::Error,
                format!("\"{}\" references missing asset {asset}", e.name),
            ),
            Some(entry) if entry.kind != want => d(
                Severity::Error,
                format!(
                    "\"{}\" needs a {} but {} is a {}",
                    e.name,
                    want.label(),
                    entry.relative,
                    entry.kind.label()
                ),
            ),
            Some(entry) if failed.contains(&asset) => d(
                Severity::Warning,
                format!(
                    "\"{}\" uses {} which failed to import; {}",
                    e.name,
                    entry.relative,
                    if want == AssetKind::Audio {
                        "it stays silent"
                    } else {
                        "a fallback is drawn"
                    }
                ),
            ),
            Some(_) => {}
        }
    }
    for (entity, param, target) in scene.dangling_entity_refs() {
        let mut d = Diagnostic::new(
            Severity::Warning,
            file.clone(),
            format!("behaviour parameter {param} names entity {target}, which does not exist"),
        );
        d.entity = Some(entity);
        out.push(d);
    }
    out
}

/// Checks every animator against the clips of its entity's model.
/// Models that failed to import are already reported elsewhere.
pub fn check_animators(
    scene: &Scene,
    assets: &[AssetSummary],
    file: Option<String>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for e in &scene.entities {
        let (Some(animator), Some(mesh)) = (&e.animator, &e.mesh) else {
            continue;
        };
        let MeshSource::Model { asset } = mesh.source else {
            continue;
        };
        let Some(summary) = assets.iter().find(|a| a.id == asset && a.imported) else {
            continue;
        };
        let problem = if summary.joints.is_none() {
            Some(format!(
                "\"{}\" has an animator but {} has no skin",
                e.name, summary.path
            ))
        } else if !summary.clips.contains(&animator.clip) {
            Some(format!(
                "\"{}\" plays clip `{}`, which {} does not have (clips: {})",
                e.name,
                animator.clip,
                summary.path,
                if summary.clips.is_empty() {
                    "none".to_owned()
                } else {
                    summary.clips.join(", ")
                }
            ))
        } else {
            None
        };
        if let Some(message) = problem {
            let mut d = Diagnostic::new(Severity::Warning, file.clone(), message);
            d.entity = Some(e.id);
            d.asset = Some(asset);
            out.push(d);
        }
    }
    out
}

/// Full project check: project file, asset metadata, every import, every
/// scene's contents and references. Needs no GPU or window.
pub fn validate_project(root: &Path) -> ValidationReport {
    let mut diagnostics = Vec::new();
    let mut assets = Vec::new();
    let mut scenes = Vec::new();
    let project = match Project::open(root) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                Some(PROJECT_FILE.to_owned()),
                e.to_string(),
            ));
            return finish(root, diagnostics, assets, scenes);
        }
    };
    let db = AssetDatabase::scan(&project);
    diagnostics.extend(db.diagnostics.iter().cloned());
    let mut failed = Vec::new();
    for entry in &db.entries {
        let mut summary = AssetSummary {
            id: entry.id,
            path: entry.relative.clone(),
            kind: entry.kind,
            imported: false,
            triangles: None,
            texture_size: None,
            duration_secs: None,
            joints: None,
            clips: Vec::new(),
        };
        match AssetDatabase::import(&project, entry) {
            Ok(imported) => {
                summary.imported = true;
                for w in imported.warnings() {
                    let mut d =
                        Diagnostic::new(Severity::Warning, Some(entry.relative.clone()), w.clone());
                    d.asset = Some(entry.id);
                    diagnostics.push(d);
                }
                match imported {
                    ImportedAsset::Model(m) => {
                        summary.triangles = Some(m.triangle_count());
                        summary.joints = m.skeleton.as_ref().map(|s| s.joints.len());
                        summary.clips = m.clips.iter().map(|c| c.name.clone()).collect();
                    }
                    ImportedAsset::Texture(t) => summary.texture_size = Some((t.width, t.height)),
                    ImportedAsset::Audio(a) => summary.duration_secs = Some(a.duration_secs),
                }
            }
            Err(e) => {
                failed.push(entry.id);
                let mut d =
                    Diagnostic::new(Severity::Error, Some(entry.relative.clone()), e.to_string());
                d.asset = Some(entry.id);
                diagnostics.push(d);
            }
        }
        assets.push(summary);
    }
    let startup = project.startup_scene_path();
    if !startup.is_file() {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            Some(PROJECT_FILE.to_owned()),
            format!(
                "startup scene {} does not exist",
                project.file.startup_scene
            ),
        ));
    }
    for path in project.scene_files() {
        let rel = project.relative(&path);
        match copperframe_scene::load_scene(&path) {
            Ok(scene) => {
                diagnostics.extend(check_scene_refs(&scene, &db, &failed, Some(rel.clone())));
                diagnostics.extend(check_animators(&scene, &assets, Some(rel.clone())));
                scenes.push(SceneSummary {
                    path: rel,
                    revision: Some(scene.revision),
                    entities: Some(scene.entities.len()),
                });
            }
            Err(e) => {
                diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    Some(rel.clone()),
                    e.to_string(),
                ));
                scenes.push(SceneSummary {
                    path: rel,
                    revision: None,
                    entities: None,
                });
            }
        }
    }
    finish(&project.root, diagnostics, assets, scenes)
}

fn finish(
    root: &Path,
    diagnostics: Vec<Diagnostic>,
    assets: Vec<AssetSummary>,
    scenes: Vec<SceneSummary>,
) -> ValidationReport {
    ValidationReport {
        project: root.display().to_string(),
        errors: diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        warnings: diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
        diagnostics,
        assets,
        scenes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Fixture;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("psx_project_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn create_open_validate_and_reference_assets() {
        let root = temp("full");
        let project = Project::create(&root, "Test").unwrap();
        assert!(Project::create(&root, "Again").is_err());
        let report = validate_project(&root);
        assert!(report.ok(), "{:?}", report.diagnostics);
        assert_eq!(report.scenes.len(), 1);

        // An unregistered model is a warning; registering gives it an id.
        let model = root.join("assets/props/box.glb");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(
            &model,
            Fixture::textured_box([0.5; 3], [1.0; 4], None).to_glb(),
        )
        .unwrap();
        assert_eq!(validate_project(&root).warnings, 1);
        let meta = AssetDatabase::register(&project, &model).unwrap();
        assert_eq!(
            AssetDatabase::register(&project, &model).unwrap().id,
            meta.id
        );
        let report = validate_project(&root);
        assert!(
            report.ok() && report.warnings == 0,
            "{:?}",
            report.diagnostics
        );
        assert_eq!(report.assets[0].triangles, Some(12));
        assert_eq!(report.assets[0].path, "assets/props/box.glb");

        // A scene referencing it validates; a missing id does not.
        let scene_path = project.startup_scene_path();
        let mut scene = copperframe_scene::load_scene(&scene_path).unwrap();
        let mut e = Entity::new("Box");
        e.mesh = Some(MeshComponent {
            source: MeshSource::Model { asset: meta.id },
            material: Default::default(),
        });
        scene.entities.push(e.clone());
        copperframe_scene::save_scene(&scene, &scene_path).unwrap();
        assert!(validate_project(&root).ok());
        let db = AssetDatabase::scan(&project);
        let mut bad = scene.clone();
        bad.entities[1].mesh = Some(MeshComponent {
            source: MeshSource::Model {
                asset: AssetId::new(),
            },
            material: Default::default(),
        });
        let diags = check_scene_refs(&bad, &db, &[], None);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing asset"));
        bad.entities[1].mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Cube,
                texture: Some(meta.id),
                tile_size: None,
            },
            material: Default::default(),
        });
        assert!(
            check_scene_refs(&bad, &db, &[], None)[0]
                .message
                .contains("needs a texture")
        );

        // Moving the file with its sidecar keeps the id.
        std::fs::rename(&model, root.join("assets/box2.glb")).unwrap();
        std::fs::rename(
            meta::meta_path(&model),
            root.join("assets/box2.glb.meta.json"),
        )
        .unwrap();
        let report = validate_project(&root);
        assert!(report.ok(), "{:?}", report.diagnostics);
        assert_eq!(report.assets[0].id, meta.id);

        // Breaking the model: the import error is reported and the scene
        // reference becomes a warning (fallback), not a crash.
        std::fs::write(root.join("assets/box2.glb"), b"broken").unwrap();
        let report = validate_project(&root);
        assert_eq!(report.errors, 1, "{:?}", report.diagnostics);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("fallback"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn duplicate_ids_orphans_and_lock() {
        let root = temp("dups");
        let project = Project::create(&root, "T").unwrap();
        let a = root.join("assets/a.glb");
        std::fs::write(&a, Fixture::textured_box([0.5; 3], [1.0; 4], None).to_glb()).unwrap();
        AssetDatabase::register(&project, &a).unwrap();
        std::fs::copy(&a, root.join("assets/b.glb")).unwrap();
        std::fs::copy(meta::meta_path(&a), root.join("assets/b.glb.meta.json")).unwrap();
        std::fs::copy(meta::meta_path(&a), root.join("assets/gone.glb.meta.json")).unwrap();
        let db = AssetDatabase::scan(&project);
        assert_eq!(db.entries.len(), 1);
        let text: Vec<String> = db.diagnostics.iter().map(|d| d.to_string()).collect();
        assert!(text.iter().any(|t| t.contains("also used by")), "{text:?}");
        assert!(
            text.iter().any(|t| t.contains("missing source")),
            "{text:?}"
        );

        let outside = root.join("x.glb");
        std::fs::copy(&a, &outside).unwrap();
        assert!(matches!(
            AssetDatabase::register(&project, &outside),
            Err(ProjectError::OutsideAssets(_))
        ));

        let lock = EditorLock::acquire(&project, false).unwrap();
        assert!(matches!(
            project.check_unlocked(),
            Err(ProjectError::Locked { .. })
        ));
        assert!(EditorLock::acquire(&project, false).is_err());
        drop(lock);
        assert!(project.check_unlocked().is_ok());
        assert!(!project.lock_path().exists());

        // A lock file nobody holds (the editor crashed) is ignored and
        // replaced without --steal-lock.
        std::fs::write(project.lock_path(), "4242").unwrap();
        assert!(project.check_unlocked().is_ok());
        let lock = EditorLock::acquire(&project, false).unwrap();
        let pid = project.editor_lock().expect("held");
        // Windows does not let another handle read a locked file.
        assert!(pid == std::process::id() || (cfg!(windows) && pid == 0));
        assert!(project.check_unlocked().is_err());
        drop(lock);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_file_is_checked() {
        let root = temp("pf");
        assert!(matches!(
            Project::open(&root),
            Err(ProjectError::NotAProject(_))
        ));
        let mut p = Project::create(&root, "T").unwrap();
        p.file.physics.gravity = glam::Vec3::new(0.0, -3.0, 0.0);
        p.save().unwrap();
        let text = std::fs::read_to_string(root.join(PROJECT_FILE)).unwrap();
        assert!(text.contains("\"gravity\""), "{text}");
        assert_eq!(Project::open(&root).unwrap().file.physics, p.file.physics);
        p.file.physics.gravity = glam::Vec3::new(0.0, f32::INFINITY, 0.0);
        std::fs::write(root.join(PROJECT_FILE), text.replace("-3.0", "1e9")).unwrap();
        assert!(Project::open(&root).is_err());
        p.file.physics = PhysicsSettings::default();
        p.file.startup_scene = "../outside.scene.json".into();
        p.save().unwrap();
        assert!(Project::open(&root).is_err());
        assert!(!validate_project(&root).ok());
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn version_1_project_files_load_with_default_settings() {
        let root = temp("v1");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/v1.project.json"),
            root.join(PROJECT_FILE),
        )
        .unwrap();
        let mut p = Project::open(&root).unwrap();
        assert_eq!(p.file.version, 1);
        assert_eq!(p.file.name, "Example room");
        assert_eq!(p.file.display, DisplaySettings::default());
        assert_eq!(p.file.style, StyleSettings::default());
        assert_eq!(p.file.build, BuildSettings::default());
        assert_eq!(p.file.physics, PhysicsSettings::default());
        assert_eq!(p.export_dir(), root.join("dist/Example_room"));
        assert_eq!(p.export_check_script(), None);

        // Saving writes the current version with every section.
        p.file.display.width = 640;
        p.file.style.default = GraphicsStyle::Ps1;
        p.file.build.export_dir = "../builds/room".into();
        p.save().unwrap();
        let text = std::fs::read_to_string(root.join(PROJECT_FILE)).unwrap();
        assert!(text.contains("\"version\": 6") && text.contains("\"look\""));
        assert!(text.contains("\"audio\"") && text.contains("\"scripting\""));
        let again = Project::open(&root).unwrap();
        p.file.version = PROJECT_VERSION;
        assert_eq!(again.file, p.file);
        assert_eq!(again.export_dir(), root.join("../builds/room"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn version_2_project_files_load_with_the_ps1_project_look() {
        let root = temp("v2");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/v2.project.json"),
            root.join(PROJECT_FILE),
        )
        .unwrap();
        let p = Project::open(&root).unwrap();
        assert_eq!(p.file.version, 2);
        assert_eq!(p.file.style.default, GraphicsStyle::Ps2);
        assert!(!p.file.style.player_can_change);
        assert_eq!(p.file.style.look, copperframe_core::RetroStyle::ps1());
        assert_eq!(p.file.physics.gravity.y, -5.0);
        assert_eq!(p.file.audio, AudioSettings::default());
        let mut f = p.file.clone();
        f.audio.music.volume_db = 40.0;
        assert!(f.check().unwrap_err().contains("audio.music"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn version_4_project_files_load_with_lua_scripting_on() {
        let root = temp("v4");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/v4.project.json"),
            root.join(PROJECT_FILE),
        )
        .unwrap();
        let p = Project::open(&root).unwrap();
        assert_eq!(p.file.version, 4);
        assert_eq!(p.file.audio.music.volume_db, -6.0);
        assert_eq!(
            p.file.scripting,
            ScriptingSettings {
                lua: true,
                csharp: false
            }
        );
        p.save().unwrap();
        let text = std::fs::read_to_string(root.join(PROJECT_FILE)).unwrap();
        assert!(text.contains("\"version\": 6") && text.contains("\"scripting\""));
        let again = Project::open(&root).unwrap();
        assert_eq!(again.file.scripting, p.file.scripting);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn version_5_project_files_load_with_csharp_off() {
        let root = temp("v5");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/v5.project.json"),
            root.join(PROJECT_FILE),
        )
        .unwrap();
        let p = Project::open(&root).unwrap();
        assert_eq!(p.file.version, 5);
        assert!(!p.file.scripting.lua && !p.file.scripting.csharp);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_settings_are_checked() {
        let root = temp("settings");
        let p = Project::create(&root, "T").unwrap();
        let mut f = p.file.clone();
        assert!(f.check().is_ok());
        f.display.width = 10;
        assert!(f.check().unwrap_err().contains("display size"));
        f = p.file.clone();
        f.name = "  ".into();
        assert!(f.check().is_err());
        f = p.file.clone();
        f.build.check_script = "/abs/x.script.json".into();
        assert!(f.check().is_err());
        // A bad value in the file stops it opening with a clear message.
        let text = std::fs::read_to_string(root.join(PROJECT_FILE))
            .unwrap()
            .replace("\"height\": 960", "\"height\": 5");
        std::fs::write(root.join(PROJECT_FILE), text).unwrap();
        let err = Project::open(&root).unwrap_err().to_string();
        assert!(err.contains("display size"), "{err}");
        // So does a newer version.
        let text = std::fs::read_to_string(root.join(PROJECT_FILE))
            .unwrap()
            .replace("\"version\": 6", "\"version\": 7");
        std::fs::write(root.join(PROJECT_FILE), text).unwrap();
        assert!(
            Project::open(&root)
                .unwrap_err()
                .to_string()
                .contains("version 7")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
