//! Undo/redo for authoring commands, with dirty-state tracking.

use crate::command::{Change, Command, CommandError, apply};
use crate::scene::Scene;

/// Default number of undo steps kept.
pub const DEFAULT_LIMIT: usize = 256;

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    change: Change,
    /// Identifies a continuous edit (e.g. one drag) that merges into one step.
    merge_key: Option<String>,
    serial: u64,
}

/// Undo and redo stacks. Every edit goes through [`History::execute`].
#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    next_serial: u64,
    /// Serial of the top undo entry when the scene was last saved (0 = the
    /// state with no entries), or `None` if that state is unreachable.
    saved_at: Option<u64>,
    merge_open: bool,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            next_serial: 1,
            saved_at: Some(0),
            merge_open: false,
        }
    }

    fn top_serial(&self) -> u64 {
        self.undo.last().map_or(0, |e| e.serial)
    }

    /// True when the scene differs from the last saved (or loaded) state.
    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.top_serial())
    }

    /// Records that the current state has been saved.
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.top_serial());
        self.merge_open = false;
    }

    /// Applies `command` and records it as one undo step.
    pub fn execute(
        &mut self,
        scene: &mut Scene,
        command: &Command,
    ) -> Result<Change, CommandError> {
        self.execute_merging(scene, command, None)
    }

    /// Like [`History::execute`], but consecutive commands with the same
    /// `merge_key` (until [`History::end_merge`]) form a single undo step.
    /// Use it for drags and typing so one gesture undoes at once.
    pub fn execute_merging(
        &mut self,
        scene: &mut Scene,
        command: &Command,
        merge_key: Option<&str>,
    ) -> Result<Change, CommandError> {
        let change = apply(scene, command)?;
        if change.is_empty() {
            return Ok(change);
        }
        self.redo.clear();
        if let (true, Some(key), Some(top)) = (self.merge_open, merge_key, self.undo.last_mut())
            && top.merge_key.as_deref() == Some(key)
            && self.saved_at != Some(top.serial)
            && let Some(merged) = top.change.merge(&change)
        {
            top.change = merged;
            return Ok(change);
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        self.undo.push(Entry {
            label: describe(command),
            change: change.clone(),
            merge_key: merge_key.map(str::to_owned),
            serial,
        });
        self.merge_open = merge_key.is_some();
        if self.undo.len() > self.limit {
            // The state with an empty undo stack is now the one after the
            // dropped step; the state before it can no longer be reached.
            let dropped = self.undo.remove(0);
            if self.saved_at == Some(0) {
                self.saved_at = None;
            } else if self.saved_at == Some(dropped.serial) {
                self.saved_at = Some(0);
            }
        }
        Ok(change)
    }

    /// Ends the current merge group; the next edit starts a new undo step.
    pub fn end_merge(&mut self) {
        self.merge_open = false;
    }

    /// Reverts the last step. Returns its label.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<String> {
        self.merge_open = false;
        let entry = self.undo.pop()?;
        entry.change.revert(scene);
        let label = entry.label.clone();
        self.redo.push(entry);
        Some(label)
    }

    /// Re-applies the last undone step. Returns its label.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<String> {
        self.merge_open = false;
        let entry = self.redo.pop()?;
        entry.change.reapply(scene);
        let label = entry.label.clone();
        self.undo.push(entry);
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Forgets all steps (e.g. after loading another scene).
    pub fn clear(&mut self) {
        *self = Self::new(self.limit);
    }
}

fn describe(command: &Command) -> String {
    match command {
        Command::CreateEntity { name, .. } => format!("create \"{name}\""),
        Command::Rename { name, .. } => format!("rename to \"{name}\""),
        other => other.op().replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Transform;
    use glam::Vec3;
    use psx_core::EntityId;

    fn create(name: &str, parent: Option<EntityId>) -> Command {
        Command::CreateEntity {
            id: Some(EntityId::new()),
            name: name.into(),
            parent,
            transform: None,
            mesh: None,
        }
    }

    fn id_of(cmd: &Command) -> EntityId {
        match cmd {
            Command::CreateEntity { id: Some(id), .. } => *id,
            _ => unreachable!(),
        }
    }

    fn move_to(id: EntityId, x: f32) -> Command {
        Command::SetTransform {
            id,
            translation: Some(Vec3::new(x, 0.0, 0.0)),
            rotation: None,
            rotation_euler_deg: None,
            scale: None,
        }
    }

    #[test]
    fn undo_all_restores_start_and_redo_all_restores_end() {
        let mut scene = Scene::new("t");
        let start = scene.clone();
        let mut h = History::default();
        let a = create("a", None);
        let b = create("b", Some(id_of(&a)));
        let c = create("c", None);
        let script = vec![
            a.clone(),
            b.clone(),
            c.clone(),
            move_to(id_of(&b), 3.0),
            Command::SetParent {
                id: id_of(&c),
                parent: Some(id_of(&b)),
                keep_world_transform: true,
            },
            Command::Rename {
                id: id_of(&a),
                name: "renamed".into(),
            },
            Command::DuplicateEntity {
                id: id_of(&a),
                new_id: None,
            },
            Command::DeleteEntity { id: id_of(&b) },
        ];
        let mut states = vec![scene.clone()];
        for cmd in &script {
            h.execute(&mut scene, cmd).unwrap();
            states.push(scene.clone());
        }
        let end = scene.clone();
        for expected in states.iter().rev().skip(1) {
            h.undo(&mut scene).unwrap();
            assert_eq!(&scene, expected);
            assert!(scene.problems().is_empty());
        }
        assert_eq!(scene, start);
        assert!(h.undo(&mut scene).is_none());
        for expected in states.iter().skip(1) {
            h.redo(&mut scene).unwrap();
            assert_eq!(&scene, expected);
        }
        assert_eq!(scene, end);
        assert!(h.redo(&mut scene).is_none());
    }

    #[test]
    fn merged_drag_is_one_step_and_new_edit_clears_redo() {
        let mut scene = Scene::new("t");
        let mut h = History::default();
        let a = create("a", None);
        let id = id_of(&a);
        h.execute(&mut scene, &a).unwrap();
        for x in 1..=10 {
            h.execute_merging(&mut scene, &move_to(id, x as f32), Some("drag"))
                .unwrap();
        }
        h.end_merge();
        h.execute_merging(&mut scene, &move_to(id, 20.0), Some("drag"))
            .unwrap();
        assert_eq!(scene.get(id).unwrap().transform.translation.x, 20.0);
        h.undo(&mut scene);
        assert_eq!(scene.get(id).unwrap().transform.translation.x, 10.0);
        h.undo(&mut scene);
        assert_eq!(scene.get(id).unwrap().transform, Transform::IDENTITY);
        assert!(h.can_redo());
        h.execute(&mut scene, &move_to(id, 1.0)).unwrap();
        assert!(!h.can_redo());
    }

    #[test]
    fn dirty_tracking_follows_undo_and_redo() {
        let mut scene = Scene::new("t");
        let mut h = History::default();
        assert!(!h.is_dirty());
        let a = create("a", None);
        h.execute(&mut scene, &a).unwrap();
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        h.execute(&mut scene, &move_to(id_of(&a), 1.0)).unwrap();
        assert!(h.is_dirty());
        h.undo(&mut scene);
        assert!(!h.is_dirty());
        h.undo(&mut scene);
        assert!(h.is_dirty());
        h.redo(&mut scene);
        assert!(!h.is_dirty());
        // A no-op command adds no step and does not dirty the scene.
        h.execute(&mut scene, &move_to(id_of(&a), 0.0)).unwrap();
        assert!(!h.is_dirty());
        // Merging never folds edits into the saved step.
        h.execute_merging(&mut scene, &move_to(id_of(&a), 2.0), Some("k"))
            .unwrap();
        h.mark_saved();
        h.execute_merging(&mut scene, &move_to(id_of(&a), 3.0), Some("k"))
            .unwrap();
        assert!(h.is_dirty());
        h.undo(&mut scene);
        assert!(!h.is_dirty());
        assert_eq!(scene.get(id_of(&a)).unwrap().transform.translation.x, 2.0);
    }

    #[test]
    fn limit_drops_oldest_steps() {
        let mut scene = Scene::new("t");
        let mut h = History::new(3);
        let a = create("a", None);
        h.execute(&mut scene, &a).unwrap();
        for x in 1..=5 {
            h.execute(&mut scene, &move_to(id_of(&a), x as f32))
                .unwrap();
        }
        let mut undone = 0;
        while h.undo(&mut scene).is_some() {
            undone += 1;
        }
        assert_eq!(undone, 3);
        assert_eq!(scene.get(id_of(&a)).unwrap().transform.translation.x, 2.0);
        assert!(
            h.is_dirty(),
            "the saved (empty) state is no longer reachable"
        );
    }
}
