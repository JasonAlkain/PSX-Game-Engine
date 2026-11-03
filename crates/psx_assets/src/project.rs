//! Project layout, asset database, editor lock and validation.
//!
//! ```text
//! my_game/
//!   project.json          {"format": "psx_project", "version": 1, ...}
//!   assets/               source files + <source>.meta.json sidecars
//!   scenes/               *.scene.json
//!   .psx/editor.lock      present while an editor has the project open
//! ```
//!
//! Paths stored in project files are relative to the project root and use
//! `/` separators, so projects move between machines and operating systems.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use psx_core::{AssetId, EntityId};
use psx_scene::{
    BehaviorComponent, BodyKind, ColliderComponent, ColliderShape, Entity, MeshComponent,
    MeshSource, PrimitiveShape, Scene,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::import::{CpuTexture, ImportError, ImportedModel};
use crate::meta::{self, AssetKind, AssetMeta, ImportSettings, MetaError};

pub const PROJECT_FILE: &str = "project.json";
pub const PROJECT_FORMAT: &str = "psx_project";
pub const PROJECT_VERSION: u32 = 1;
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
    #[error("no project at {0} (missing {PROJECT_FILE}); create one with `psx new`")]
    NotAProject(PathBuf),
    #[error("{path} is not a valid project file: {reason}")]
    Invalid { path: PathBuf, reason: String },
    #[error(transparent)]
    Scene(#[from] psx_scene::SceneFileError),
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error("{0} is not a supported asset type (glTF/GLB, PNG, JPEG)")]
    UnsupportedAsset(PathBuf),
    #[error("{0} is outside the project's assets folder")]
    OutsideAssets(PathBuf),
    #[error(
        "the project is open in an editor (lock file {path}, pid {pid}). Close the editor first, \
         or if it crashed, delete the lock file"
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
        };
        let project = Project {
            root: root.to_path_buf(),
            file,
        };
        let mut scene = Scene::new("Main");
        let mut floor = Entity::new("Floor");
        floor.transform.scale = glam::Vec3::new(8.0, 1.0, 8.0);
        floor.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Plane,
                texture: None,
            },
            material: Default::default(),
        });
        floor.collider = Some(ColliderComponent {
            offset: glam::Vec3::new(0.0, -0.1, 0.0),
            ..ColliderComponent::new(
                ColliderShape::Box {
                    size: glam::Vec3::new(1.0, 0.2, 1.0),
                },
                BodyKind::Static,
            )
        });
        let mut cube = Entity::new("Cube");
        cube.transform.translation = glam::Vec3::new(0.0, 0.5, -1.5);
        cube.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Cube,
                texture: None,
            },
            material: Default::default(),
        });
        cube.collider = Some(ColliderComponent::new(
            ColliderShape::Box {
                size: glam::Vec3::ONE,
            },
            BodyKind::Kinematic,
        ));
        cube.behaviors = vec![BehaviorComponent::new("psx.spin")];
        let mut player = Entity::new("Player");
        player.transform.translation = glam::Vec3::new(0.0, 0.0, 2.5);
        player.behaviors = vec![BehaviorComponent::new("psx.player")];
        scene.entities.extend([floor, cube, player]);
        psx_scene::save_scene(&scene, &project.startup_scene_path())?;
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
        if !is_safe_relative(&file.startup_scene) {
            return Err(invalid(format!(
                "startup_scene `{}` must be a relative path inside the project",
                file.startup_scene
            )));
        }
        Ok(Project {
            root: root.to_path_buf(),
            file,
        })
    }

    pub fn save(&self) -> Result<(), ProjectError> {
        let path = self.root.join(PROJECT_FILE);
        let mut text =
            serde_json::to_string_pretty(&self.file).map_err(|e| ProjectError::Invalid {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        text.push('\n');
        psx_core::fsutil::atomic_write(&path, text.as_bytes()).map_err(io_err(&path))
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

    /// The pid recorded in an existing editor lock.
    pub fn editor_lock(&self) -> Option<u32> {
        let text = std::fs::read_to_string(self.lock_path()).ok()?;
        Some(text.trim().parse().unwrap_or(0))
    }

    /// Fails with [`ProjectError::Locked`] while an editor has the project open.
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

/// Held by the editor while a project is open. Removing the lock file is
/// how the CLI knows it may write scene files.
#[derive(Debug)]
pub struct EditorLock {
    path: PathBuf,
}

impl EditorLock {
    /// Creates the lock. With `steal`, an existing lock (e.g. left by a
    /// crashed editor) is replaced.
    pub fn acquire(project: &Project, steal: bool) -> Result<EditorLock, ProjectError> {
        let path = project.lock_path();
        if !steal {
            project.check_unlocked()?;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        std::fs::write(&path, std::process::id().to_string()).map_err(io_err(&path))?;
        Ok(EditorLock { path })
    }
}

impl Drop for EditorLock {
    fn drop(&mut self) {
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
                     registered with `psx import` or the editor's Rescan",
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
                    ImportedAsset::Model(m) => summary.triangles = Some(m.triangle_count()),
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
        match psx_scene::load_scene(&path) {
            Ok(scene) => {
                diagnostics.extend(check_scene_refs(&scene, &db, &failed, Some(rel.clone())));
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
        let mut scene = psx_scene::load_scene(&scene_path).unwrap();
        let mut e = Entity::new("Box");
        e.mesh = Some(MeshComponent {
            source: MeshSource::Model { asset: meta.id },
            material: Default::default(),
        });
        scene.entities.push(e.clone());
        psx_scene::save_scene(&scene, &scene_path).unwrap();
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
        p.file.startup_scene = "../outside.scene.json".into();
        p.save().unwrap();
        assert!(Project::open(&root).is_err());
        assert!(!validate_project(&root).ok());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
