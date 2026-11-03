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
    Command, History, MeshComponent, MeshSource, PrimitiveShape, Scene, SceneFileError, Transform,
    load_scene, save_scene,
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
    /// Revision of the file on disk when it was loaded or last saved.
    disk_revision: u64,
    _lock: Option<EditorLock>,
}

/// Most log lines kept.
const LOG_LIMIT: usize = 500;

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
            _lock: lock,
        };
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
                } else if p.to_string_lossy().ends_with(".scene.json") {
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
        self.info(format!(
            "opened {} (revision {}, {} entities)",
            self.project.relative(&self.scene_path),
            self.scene.revision,
            self.scene.entities.len()
        ));
        Ok(())
    }

    /// Creates `scenes/<name>.scene.json` with the current scene's style,
    /// lighting and camera and no entities, and opens it.
    pub fn new_scene(&mut self, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the scene needs a name".into());
        }
        let stem: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let path = self
            .project
            .resolve(SCENES_DIR)
            .join(format!("{stem}.scene.json"));
        if path.exists() {
            return Err(format!("{} already exists", self.project.relative(&path)));
        }
        let mut scene = Scene::new(name);
        scene.settings = self.scene.settings.clone();
        save_scene(&scene, &path).map_err(|e| e.to_string())?;
        self.open_scene(&path).map_err(|e| e.to_string())?;
        Ok(path)
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
        assert_eq!(s.scene.entities.len(), 3);
        assert!(!s.is_startup_scene());
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
