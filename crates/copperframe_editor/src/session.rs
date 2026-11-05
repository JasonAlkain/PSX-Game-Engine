//! Editor state without any UI or GPU: the open project and scene, undo
//! history, selection and the console log.
//!
//! Every edit goes through [`EditorSession::execute`], i.e. the same
//! [`copperframe_scene::Command`] layer the CLI uses, so both apply identical
//! validation and serialisation rules.

use std::path::{Path, PathBuf};

use copperframe_assets::project::{AssetDatabase, EditorLock, Project, ProjectError, SCENES_DIR};
use copperframe_core::{AssetId, EntityId};
use copperframe_scene::{
    BehaviorComponent, ColliderComponent, ColliderShape, Command, Entity, History, MeshComponent,
    MeshSource, PrimitiveShape, Scene, SceneFileError, SceneSettings, Transform, load_scene,
    save_scene,
};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

/// What [`EditorSession::save`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveOutcome {
    Saved {
        revision: u64,
    },
    /// The file on disk changed since it was loaded (e.g. by the CLI with
    /// the lock removed). Nothing was written; call again with `force`.
    Conflict {
        disk_revision: u64,
    },
}

pub struct EditorSession {
    pub project: Project,
    pub db: AssetDatabase,
    pub scene: Scene,
    pub scene_path: PathBuf,
    pub history: History,
    pub selection: Option<EntityId>,
    pub log: Vec<LogLine>,
    /// Recently opened scenes, relative to the project, newest first
    /// (kept in `.psx/recent_scenes.json`).
    pub recent_scenes: Vec<String>,
    /// Revision of the file on disk when it was loaded or last saved.
    disk_revision: u64,
    _lock: Option<EditorLock>,
}

/// Most log lines kept.
const LOG_LIMIT: usize = 500;
/// Scene files end in this.
pub const SCENE_SUFFIX: &str = ".scene.json";
/// Recently opened scenes of the project.
const RECENT_FILE: &str = ".psx/recent_scenes.json";
/// Deleted scene files are moved here.
pub const DELETED_DIR: &str = ".psx/deleted";
const RECENT_SCENES: usize = 10;

/// What a new scene starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneTemplate {
    /// No entities.
    #[default]
    Empty,
    /// A floor with a collider and a player.
    Basic,
}

impl SceneTemplate {
    pub const ALL: [SceneTemplate; 2] = [SceneTemplate::Empty, SceneTemplate::Basic];

    pub fn label(self) -> &'static str {
        match self {
            SceneTemplate::Empty => "Empty",
            SceneTemplate::Basic => "Basic: floor and player",
        }
    }

    /// A new scene called `name` with the look and lighting of `settings`
    /// and the default start camera.
    pub fn build(self, name: &str, settings: &SceneSettings) -> Scene {
        let mut scene = Scene::new(name);
        scene.settings = SceneSettings {
            camera: Default::default(),
            ..settings.clone()
        };
        if self == SceneTemplate::Basic {
            let mut floor = Entity::new("Floor");
            floor.transform.scale = Vec3::new(8.0, 1.0, 8.0);
            floor.mesh = Some(MeshComponent {
                source: MeshSource::Primitive {
                    shape: PrimitiveShape::Plane,
                    texture: None,
                    tile_size: Some(2.0),
                },
                material: Default::default(),
            });
            floor.collider = Some(ColliderComponent {
                offset: Vec3::new(0.0, -0.1, 0.0),
                ..ColliderComponent::new(ColliderShape::Box {
                    size: Vec3::new(1.0, 0.2, 1.0),
                })
            });
            let mut player = Entity::new("Player");
            player.transform.translation = Vec3::new(0.0, 0.0, 2.5);
            player.behaviors = vec![BehaviorComponent::new("psx.player")];
            scene.entities.extend([floor, player]);
        }
        scene
    }
}

/// A file name stem for a scene name: letters, digits, `-` and `_`, the
/// rest replaced by `_`, lower case.
pub fn scene_stem(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Replaces string behaviour parameters equal to `old` with `new`;
/// returns how many changed.
fn replace_param(
    behaviors: &mut [copperframe_scene::BehaviorComponent],
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> usize {
    let mut n = 0;
    for b in behaviors {
        for v in b.params.values_mut() {
            if v == old {
                *v = new.clone();
                n += 1;
            }
        }
    }
    n
}

impl EditorSession {
    /// Opens a project and its startup scene (or `scene`). With `lock`, the
    /// editor lock is taken (`steal_lock` replaces a stale one).
    pub fn open(
        root: &Path,
        scene: Option<&Path>,
        lock: bool,
        steal_lock: bool,
    ) -> Result<Self, OpenError> {
        let project = Project::open(root)?;
        let lock_taken = lock;
        let lock = if lock {
            Some(EditorLock::acquire(&project, steal_lock)?)
        } else {
            None
        };
        let scene_path = scene
            .map(Path::to_path_buf)
            .unwrap_or_else(|| project.startup_scene_path());
        let scene = load_scene(&scene_path)?;
        let db = AssetDatabase::scan(&project);
        let mut session = Self {
            disk_revision: scene.revision,
            project,
            db,
            scene,
            scene_path,
            history: History::default(),
            selection: None,
            log: Vec::new(),
            recent_scenes: Vec::new(),
            _lock: lock,
        };
        session.recent_scenes = std::fs::read_to_string(session.project.root.join(RECENT_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if lock_taken {
            session.remember_scene();
        }
        session.info(format!(
            "opened {} (revision {}, {} entities)",
            session.project.relative(&session.scene_path),
            session.scene.revision,
            session.scene.entities.len()
        ));
        for d in session.db.diagnostics.clone() {
            session.warn(d.to_string());
        }
        Ok(session)
    }

    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        match level {
            LogLevel::Info => tracing::info!("{text}"),
            LogLevel::Warning => tracing::warn!("{text}"),
            LogLevel::Error => tracing::error!("{text}"),
        }
        self.log.push(LogLine { level, text });
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Info, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Warning, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Error, text);
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// Applies one command as an undo step. Errors are logged; returns the
    /// IDs the command created (empty on failure).
    pub fn execute(&mut self, command: Command) -> Option<Vec<EntityId>> {
        self.execute_merging(command, None)
    }

    /// Like [`EditorSession::execute`], merging consecutive edits that share
    /// `merge_key` into one undo step (drags, typing).
    pub fn execute_merging(
        &mut self,
        command: Command,
        merge_key: Option<&str>,
    ) -> Option<Vec<EntityId>> {
        match self
            .history
            .execute_merging(&mut self.scene, &command, merge_key)
        {
            Ok(change) => {
                self.fix_selection();
                Some(change.created)
            }
            Err(e) => {
                self.error(format!("{}: {e}", command.op()));
                None
            }
        }
    }

    pub fn end_merge(&mut self) {
        self.history.end_merge();
    }

    pub fn undo(&mut self) {
        match self.history.undo(&mut self.scene) {
            Some(label) => self.info(format!("undo {label}")),
            None => self.info("nothing to undo"),
        }
        self.fix_selection();
    }

    pub fn redo(&mut self) {
        match self.history.redo(&mut self.scene) {
            Some(label) => self.info(format!("redo {label}")),
            None => self.info("nothing to redo"),
        }
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
        if let Some(id) = self.selection
            && self.scene.get(id).is_none()
        {
            self.selection = None;
        }
    }

    /// Saves the scene with the next revision. Refuses (without `force`) if
    /// the file on disk changed since it was loaded.
    pub fn save(&mut self, force: bool) -> Result<SaveOutcome, SceneFileError> {
        if !force
            && let Ok(disk) = load_scene(&self.scene_path)
            && disk.revision != self.disk_revision
        {
            return Ok(SaveOutcome::Conflict {
                disk_revision: disk.revision,
            });
        }
        let previous = self.scene.revision;
        self.scene.revision = self.disk_revision.max(previous) + 1;
        if force && let Ok(disk) = load_scene(&self.scene_path) {
            self.scene.revision = self.scene.revision.max(disk.revision + 1);
        }
        match save_scene(&self.scene, &self.scene_path) {
            Ok(()) => {
                self.disk_revision = self.scene.revision;
                self.history.mark_saved();
                let rel = self.project.relative(&self.scene_path);
                self.info(format!("saved {rel} (revision {})", self.scene.revision));
                Ok(SaveOutcome::Saved {
                    revision: self.scene.revision,
                })
            }
            Err(e) => {
                self.scene.revision = previous;
                self.error(e.to_string());
                Err(e)
            }
        }
    }

    /// Discards unsaved edits and the undo history, and loads the file again.
    pub fn reload(&mut self) -> Result<(), SceneFileError> {
        let scene = load_scene(&self.scene_path)?;
        self.disk_revision = scene.revision;
        self.scene = scene;
        self.history.clear();
        self.fix_selection();
        self.info(format!(
            "reloaded {} (revision {})",
            self.project.relative(&self.scene_path),
            self.scene.revision
        ));
        Ok(())
    }

    /// Revision of the scene file as the editor last loaded or saved it.
    pub fn disk_revision(&self) -> u64 {
        self.disk_revision
    }

    /// Every `*.scene.json` under `scenes/`, sorted.
    pub fn scene_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.to_string_lossy().ends_with(SCENE_SUFFIX) {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.project.resolve(SCENES_DIR), &mut out);
        out.sort();
        out
    }

    /// Switches to another scene file, discarding unsaved edits and the undo
    /// history of the current one (the caller asks first).
    pub fn open_scene(&mut self, path: &Path) -> Result<(), SceneFileError> {
        let scene = load_scene(path)?;
        self.disk_revision = scene.revision;
        self.scene = scene;
        self.scene_path = path.to_path_buf();
        self.history.clear();
        self.selection = None;
        self.remember_scene();
        self.info(format!(
            "opened {} (revision {}, {} entities)",
            self.project.relative(&self.scene_path),
            self.scene.revision,
            self.scene.entities.len()
        ));
        Ok(())
    }

    /// The file a new scene called `name` gets in `folder` (relative to the
    /// project, under `scenes/`). Fails on a blank name, a folder outside
    /// `scenes/`, or an existing file (scenes are never overwritten).
    pub fn scene_path_for(&self, folder: &str, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the scene needs a name".into());
        }
        let folder = folder.trim_matches('/');
        if !(folder == SCENES_DIR || folder.starts_with(&format!("{SCENES_DIR}/")))
            || folder.split('/').any(|p| p == ".." || p.is_empty())
        {
            return Err(format!("scenes go in the {SCENES_DIR} folder"));
        }
        let path = self
            .project
            .resolve(folder)
            .join(format!("{}{SCENE_SUFFIX}", scene_stem(name)));
        if path.exists() {
            return Err(format!("{} already exists", self.project.relative(&path)));
        }
        Ok(path)
    }

    /// Writes a new scene from `template` at `path` (see
    /// [`EditorSession::scene_path_for`]) and opens it. The current scene is
    /// discarded, so the caller saves or asks first.
    pub fn new_scene_at(
        &mut self,
        path: &Path,
        name: &str,
        template: SceneTemplate,
    ) -> Result<(), String> {
        if path.exists() {
            return Err(format!("{} already exists", self.project.relative(path)));
        }
        let scene = template.build(name.trim(), &self.scene.settings);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        save_scene(&scene, path).map_err(|e| e.to_string())?;
        self.open_scene(path).map_err(|e| e.to_string())
    }

    /// Creates `scenes/<name>.scene.json` with the current scene's style
    /// and lighting and no entities, and opens it.
    pub fn new_scene(&mut self, name: &str) -> Result<PathBuf, String> {
        let path = self.scene_path_for(SCENES_DIR, name)?;
        self.new_scene_at(&path, name, SceneTemplate::Empty)?;
        Ok(path)
    }

    /// Saves the open scene, edits included, as a new file (with a new
    /// scene ID) and continues editing that file. The old file is left as
    /// it was on disk.
    pub fn save_scene_as(&mut self, path: &Path) -> Result<(), String> {
        if path.exists() {
            return Err(format!("{} already exists", self.project.relative(path)));
        }
        if !path.starts_with(self.project.resolve(SCENES_DIR)) {
            return Err(format!("scenes go in the {SCENES_DIR} folder"));
        }
        let mut scene = self.scene.clone();
        scene.id = copperframe_core::SceneId::new();
        scene.revision = 1;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        save_scene(&scene, path).map_err(|e| e.to_string())?;
        self.scene = scene;
        self.scene_path = path.to_path_buf();
        self.disk_revision = 1;
        self.history.mark_saved();
        self.remember_scene();
        self.info(format!("saved as {}", self.project.relative(path)));
        Ok(())
    }

    /// Renames or moves a scene file (`to` is the new path, under
    /// `scenes/`). The start scene, the recent list and behaviour
    /// parameters naming the old path (in every scene) follow it; in the
    /// open scene that is an undoable edit. Returns how many references
    /// were updated.
    pub fn move_scene(&mut self, from: &Path, to: &Path) -> Result<usize, String> {
        let scenes = self.project.resolve(SCENES_DIR);
        if !to.starts_with(&scenes) || !to.to_string_lossy().ends_with(SCENE_SUFFIX) {
            return Err(format!(
                "scene files must stay in {SCENES_DIR}/ and end in {SCENE_SUFFIX}"
            ));
        }
        if to.exists() {
            return Err(format!("{} already exists", self.project.relative(to)));
        }
        if !from.is_file() {
            return Err(format!("{} does not exist", self.project.relative(from)));
        }
        let old_rel = self.project.relative(from);
        let new_rel = self.project.relative(to);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::rename(from, to).map_err(|e| format!("could not move {old_rel}: {e}"))?;
        // The backup follows the file it belongs to.
        let bak = |p: &Path| PathBuf::from(format!("{}.bak", p.display()));
        if bak(from).is_file() {
            let _ = std::fs::rename(bak(from), bak(to));
        }
        let mut updated = 0;
        if self.project.file.startup_scene == old_rel {
            self.project.file.startup_scene = new_rel.clone();
            if let Err(e) = self.project.save() {
                self.error(format!("could not update the start scene: {e}"));
            } else {
                updated += 1;
            }
        }
        if self.scene_path == from {
            self.scene_path = to.to_path_buf();
        }
        for r in &mut self.recent_scenes {
            if *r == old_rel {
                *r = new_rel.clone();
            }
        }
        self.save_recent();
        // Behaviour parameters that name the scene, in every scene.
        let old_value = serde_json::Value::String(old_rel.clone());
        let new_value = serde_json::Value::String(new_rel.clone());
        let fixes: Vec<(
            copperframe_core::EntityId,
            Vec<copperframe_scene::BehaviorComponent>,
        )> = self
            .scene
            .entities
            .iter()
            .filter_map(|e| {
                let mut b = e.behaviors.clone();
                let n = replace_param(&mut b, &old_value, &new_value);
                (n > 0).then_some((e.id, b))
            })
            .collect();
        for (id, behaviors) in fixes {
            if self
                .execute(Command::SetBehaviors { id, behaviors })
                .is_some()
            {
                updated += 1;
            }
        }
        for path in self.scene_files() {
            if path == self.scene_path {
                continue;
            }
            let Ok(mut scene) = load_scene(&path) else {
                continue;
            };
            let n: usize = scene
                .entities
                .iter_mut()
                .map(|e| replace_param(&mut e.behaviors, &old_value, &new_value))
                .sum();
            if n > 0 {
                scene.revision += 1;
                match save_scene(&scene, &path) {
                    Ok(()) => updated += n,
                    Err(e) => self.error(format!(
                        "could not update {}: {e}",
                        self.project.relative(&path)
                    )),
                }
            }
        }
        self.info(format!("moved {old_rel} to {new_rel}"));
        Ok(updated)
    }

    /// Moves a scene file into `.psx/deleted/` (out of the game, but
    /// recoverable by hand). The open scene and the start scene cannot be
    /// deleted.
    pub fn delete_scene(&mut self, path: &Path) -> Result<(), String> {
        let rel = self.project.relative(path);
        if path == self.scene_path {
            return Err(format!(
                "{rel} is open; open another scene before deleting it"
            ));
        }
        if path == self.project.startup_scene_path() {
            return Err(format!(
                "{rel} is the start scene; make another scene the start scene first"
            ));
        }
        let bin = self.project.root.join(DELETED_DIR);
        std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut target = bin.join(&name);
        let mut n = 1;
        while target.exists() {
            target = bin.join(format!("{n}-{name}"));
            n += 1;
        }
        std::fs::rename(path, &target).map_err(|e| format!("could not delete {rel}: {e}"))?;
        let _ = std::fs::remove_file(format!("{}.bak", path.display()));
        self.recent_scenes.retain(|r| *r != rel);
        self.save_recent();
        self.info(format!(
            "deleted {rel} (kept as {})",
            self.project.relative(&target)
        ));
        Ok(())
    }

    /// Makes a folder under `scenes/`.
    pub fn create_scene_folder(&mut self, parent: &str, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err("give the folder a plain name".into());
        }
        let parent = parent.trim_matches('/');
        if !(parent == SCENES_DIR || parent.starts_with(&format!("{SCENES_DIR}/"))) {
            return Err(format!("scene folders go in {SCENES_DIR}/"));
        }
        let path = self.project.resolve(parent).join(name);
        if path.exists() {
            return Err(format!("{} already exists", self.project.relative(&path)));
        }
        std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// Every folder under `scenes/` (relative, sorted), including empty ones.
    pub fn scene_folders(&self) -> Vec<String> {
        fn walk(dir: &Path, project: &Project, out: &mut Vec<String>) {
            out.push(project.relative(dir));
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                let hidden = e.file_name().to_string_lossy().starts_with('.');
                if p.is_dir() && !hidden {
                    walk(&p, project, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.project.resolve(SCENES_DIR), &self.project, &mut out);
        out.sort();
        out
    }

    /// Recently opened scenes of this project (relative paths, newest
    /// first), without the open one and without files that are gone.
    pub fn recent_scene_paths(&self) -> Vec<PathBuf> {
        self.recent_scenes
            .iter()
            .map(|r| self.project.resolve(r))
            .filter(|p| *p != self.scene_path && p.is_file())
            .collect()
    }

    /// Puts the open scene at the top of the recent list.
    fn remember_scene(&mut self) {
        let rel = self.project.relative(&self.scene_path);
        self.recent_scenes.retain(|r| *r != rel);
        self.recent_scenes.insert(0, rel);
        self.recent_scenes.truncate(RECENT_SCENES);
        self.save_recent();
    }

    fn save_recent(&self) {
        let path = self.project.root.join(RECENT_FILE);
        let text = serde_json::to_string_pretty(&self.recent_scenes).unwrap_or_default();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = copperframe_core::fsutil::atomic_write(&path, text.as_bytes()) {
            tracing::warn!("could not save the recent scenes list: {e}");
        }
    }

    pub fn is_startup_scene(&self) -> bool {
        self.project.startup_scene_path() == self.scene_path
    }

    /// Makes the open scene the one the game starts with.
    pub fn set_startup_scene(&mut self) {
        let rel = self.project.relative(&self.scene_path);
        let previous = std::mem::replace(&mut self.project.file.startup_scene, rel.clone());
        match self.project.save() {
            Ok(()) => self.info(format!("the game now starts with {rel}")),
            Err(e) => {
                self.project.file.startup_scene = previous;
                self.error(e.to_string());
            }
        }
    }

    /// Rescans `assets/`, registering new source files.
    pub fn rescan_assets(&mut self) {
        match AssetDatabase::register_all(&self.project) {
            Ok(new) => {
                for p in new {
                    let rel = self.project.relative(&p);
                    self.info(format!("registered {rel}"));
                }
            }
            Err(e) => self.error(e.to_string()),
        }
        self.db = AssetDatabase::scan(&self.project);
        for d in self.db.diagnostics.clone() {
            self.warn(d.to_string());
        }
    }

    /// Creates an entity with a mesh at `position` and selects it.
    pub fn create_mesh_entity(&mut self, name: &str, source: Option<MeshSource>, position: Vec3) {
        let created = self.execute(Command::CreateEntity {
            id: None,
            name: name.to_owned(),
            parent: None,
            transform: Some(Transform::from_translation(position)),
            mesh: source.map(|source| MeshComponent {
                source,
                material: Default::default(),
            }),
        });
        if let Some(id) = created.and_then(|c| c.first().copied()) {
            self.selection = Some(id);
        }
    }

    pub fn create_primitive(&mut self, shape: PrimitiveShape, position: Vec3) {
        self.create_mesh_entity(
            shape.label(),
            Some(MeshSource::Primitive {
                shape,
                texture: None,
                tile_size: Some(copperframe_scene::DEFAULT_TILE_SIZE),
            }),
            position,
        );
    }

    pub fn add_model(&mut self, asset: AssetId, position: Vec3) {
        let name = self
            .db
            .get(asset)
            .and_then(|e| {
                Path::new(&e.relative)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Model".to_owned());
        self.create_mesh_entity(&name, Some(MeshSource::Model { asset }), position);
    }

    pub fn duplicate_selected(&mut self) {
        let Some(id) = self.selection else { return };
        if let Some(created) = self.execute(Command::DuplicateEntity { id, new_id: None }) {
            self.selection = created.first().copied();
        }
    }

    pub fn delete_selected(&mut self) {
        let Some(id) = self.selection else { return };
        self.execute(Command::DeleteEntity { id });
        self.selection = None;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Scene(#[from] SceneFileError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use copperframe_assets::fixtures::Fixture;
    use copperframe_scene::{CommandBatch, apply_batch};

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("psx_editor_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// Milestone 3 acceptance, without the window: a room assembled through
    /// editor operations survives a restart with the same entities and
    /// assets, can be modified through the CLI command layer, and remains
    /// valid after undo/redo.
    #[test]
    fn room_survives_restart_cli_edit_and_undo() {
        let root = temp("room");
        Project::create(&root, "Room").unwrap();
        let model = root.join("assets/crate.glb");
        std::fs::write(
            &model,
            Fixture::textured_box([0.5; 3], [1.0; 4], None).to_glb(),
        )
        .unwrap();

        let mut s = EditorSession::open(&root, None, true, false).unwrap();
        assert!(s.project.check_unlocked().is_err(), "editor holds the lock");
        s.rescan_assets();
        let asset = s.db.entries[0].id;
        s.create_primitive(PrimitiveShape::Cube, Vec3::new(0.0, 0.5, 0.0));
        let wall = s.selection.unwrap();
        s.execute(Command::Rename {
            id: wall,
            name: "Wall".into(),
        });
        for x in 0..5 {
            // A drag: five updates, one undo step.
            s.execute_merging(
                Command::SetTransform {
                    id: wall,
                    translation: None,
                    rotation: None,
                    rotation_euler_deg: None,
                    scale: Some(Vec3::new(4.0 + x as f32, 3.0, 0.2)),
                },
                Some("scale"),
            );
        }
        s.end_merge();
        s.add_model(asset, Vec3::new(1.0, 0.5, -1.0));
        let crate_id = s.selection.unwrap();
        s.execute(Command::SetParent {
            id: crate_id,
            parent: Some(wall),
            keep_world_transform: true,
        });
        s.selection = Some(crate_id);
        s.duplicate_selected();
        let copy = s.selection.unwrap();
        assert_ne!(copy, crate_id);
        assert!(s.is_dirty());

        // Undo everything back to the loaded state and redo it again: valid
        // at every step.
        let end = s.scene.clone();
        let mut steps = 0;
        while s.history.can_undo() {
            s.undo();
            assert!(s.scene.problems().is_empty());
            steps += 1;
        }
        assert_eq!(
            steps, 6,
            "create, rename, drag, add model, parent, duplicate"
        );
        assert!(!s.is_dirty());
        while s.history.can_redo() {
            s.redo();
        }
        assert_eq!(s.scene, end);
        assert_eq!(
            s.scene.get(wall).unwrap().transform.scale,
            Vec3::new(8.0, 3.0, 0.2)
        );

        assert!(matches!(
            s.save(false),
            Ok(SaveOutcome::Saved { revision: 1 })
        ));
        assert!(!s.is_dirty());
        let saved = s.scene.clone();
        drop(s);

        // "Restart": same entities, ids, transforms and asset references.
        let mut s = EditorSession::open(&root, None, true, false).unwrap();
        assert_eq!(s.scene, saved);
        assert_eq!(s.db.get(asset).unwrap().relative, "assets/crate.glb");
        drop(s);

        // CLI-equivalent batch on the closed project.
        let project = Project::open(&root).unwrap();
        project.check_unlocked().unwrap();
        let mut batch = CommandBatch::new(vec![Command::Rename {
            id: copy,
            name: "Crate B".into(),
        }]);
        batch.expected_revision = Some(1);
        let on_disk = load_scene(&project.startup_scene_path()).unwrap();
        let (edited, _) = apply_batch(&on_disk, &batch).unwrap();
        save_scene(&edited, &project.startup_scene_path()).unwrap();

        s = EditorSession::open(&root, None, true, false).unwrap();
        assert_eq!(s.scene.get(copy).unwrap().name, "Crate B");
        assert_eq!(s.scene.revision, 2);

        // A file changed behind the editor's back is not silently overwritten.
        let mut behind = load_scene(&s.scene_path).unwrap();
        behind.revision = 7;
        save_scene(&behind, &s.scene_path).unwrap();
        s.execute(Command::Rename {
            id: wall,
            name: "Wall 2".into(),
        });
        assert_eq!(
            s.save(false).unwrap(),
            SaveOutcome::Conflict { disk_revision: 7 }
        );
        assert!(matches!(
            s.save(true),
            Ok(SaveOutcome::Saved { revision: 8 })
        ));

        // A broken model does not make the scene invalid or unloadable.
        std::fs::write(&model, b"broken").unwrap();
        drop(s);
        let s = EditorSession::open(&root, None, true, false).unwrap();
        assert!(s.scene.problems().is_empty());
        let report = copperframe_assets::validate_project(&root);
        assert_eq!(
            report.errors, 1,
            "only the import error: {:?}",
            report.diagnostics
        );
        drop(s);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scenes_can_be_created_opened_and_made_startup() {
        let root = temp("scenes");
        Project::create(&root, "T").unwrap();
        let mut s = EditorSession::open(&root, None, false, false).unwrap();
        let main = s.scene_path.clone();
        assert!(s.is_startup_scene());
        let level = s.new_scene("Level 2").unwrap();
        assert!(level.ends_with("scenes/level_2.scene.json"));
        assert_eq!(s.scene_path, level);
        assert!(s.scene.entities.is_empty());
        assert!(!s.is_dirty());
        assert!(
            s.new_scene("Level 2")
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(s.scene_files(), vec![level.clone(), main.clone()]);
        s.set_startup_scene();
        assert_eq!(
            Project::open(&root).unwrap().file.startup_scene,
            "scenes/level_2.scene.json"
        );
        s.open_scene(&main).unwrap();
        assert_eq!(s.scene.entities.len(), 4);
        assert!(!s.is_startup_scene());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scenes_save_as_move_delete_and_keep_references() {
        let root = temp("scene_files");
        Project::create(&root, "T").unwrap();
        let mut s = EditorSession::open(&root, None, true, false).unwrap();
        let main = s.scene_path.clone();

        // A second scene whose behaviour names the main scene.
        let level = s.scene_path_for("scenes", "Level").unwrap();
        s.new_scene_at(&level, "Level", SceneTemplate::Basic)
            .unwrap();
        assert_eq!(
            s.scene.entities.len(),
            2,
            "basic template: floor and player"
        );
        let player = s.scene.entities[1].id;
        let mut door = copperframe_scene::BehaviorComponent::new("game.door");
        door.params.insert(
            "scene".into(),
            serde_json::Value::String("scenes/main.scene.json".into()),
        );
        s.execute(Command::SetBehaviors {
            id: player,
            behaviors: vec![door],
        });
        s.save(false).unwrap();
        // Existing files are never overwritten; names outside scenes/ are refused.
        assert!(
            s.scene_path_for("scenes", "level")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(s.scene_path_for("assets", "x").is_err());
        assert!(s.scene_path_for("scenes/../assets", "x").is_err());

        // Save As keeps edits, makes a new file and leaves the old one.
        s.execute(Command::Rename {
            id: player,
            name: "Hero".into(),
        });
        let copy = s.scene_path_for("scenes", "Level copy").unwrap();
        let old_id = s.scene.id;
        s.save_scene_as(&copy).unwrap();
        assert_eq!(s.scene_path, copy);
        assert!(!s.is_dirty());
        assert_ne!(s.scene.id, old_id);
        assert_eq!(load_scene(&copy).unwrap().entities[1].name, "Hero");
        assert_eq!(load_scene(&level).unwrap().entities[1].name, "Player");
        assert!(s.save_scene_as(&level).is_err(), "no overwrite");

        // Moving the start scene into a folder updates the project and
        // every scene that names it, open or not.
        let folder = s.create_scene_folder("scenes", "levels").unwrap();
        let moved = folder.join("main.scene.json");
        let n = s.move_scene(&main, &moved).unwrap();
        assert_eq!(n, 3, "start scene, the open copy, the closed level");
        assert_eq!(
            Project::open(&root).unwrap().file.startup_scene,
            "scenes/levels/main.scene.json"
        );
        let param = |scene: &Scene| scene.entities[1].behaviors[0].params["scene"].clone();
        assert_eq!(param(&s.scene), "scenes/levels/main.scene.json");
        assert!(
            s.is_dirty(),
            "the open scene's reference is an undoable edit"
        );
        assert_eq!(
            param(&load_scene(&level).unwrap()),
            "scenes/levels/main.scene.json"
        );
        assert!(!main.exists() && moved.exists());
        assert!(s.scene_folders().contains(&"scenes/levels".to_owned()));

        // Deleting: not the open or start scene; others go to .psx/deleted.
        assert!(s.delete_scene(&copy).is_err());
        assert!(s.delete_scene(&moved).is_err());
        s.delete_scene(&level).unwrap();
        assert!(!level.exists());
        assert!(root.join(DELETED_DIR).join("level.scene.json").is_file());

        // Recent scenes survive a restart.
        s.open_scene(&moved).unwrap();
        drop(s);
        let s = EditorSession::open(&root, Some(&copy), true, false).unwrap();
        assert_eq!(s.recent_scene_paths(), vec![moved.clone()]);
        drop(s);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_edits_are_logged_not_applied() {
        let root = temp("fail");
        Project::create(&root, "T").unwrap();
        let mut s = EditorSession::open(&root, None, false, false).unwrap();
        let before = s.scene.clone();
        assert!(
            s.execute(Command::DeleteEntity {
                id: EntityId::new()
            })
            .is_none()
        );
        assert_eq!(s.scene, before);
        assert_eq!(s.log.last().unwrap().level, LogLevel::Error);
        assert!(!s.is_dirty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
