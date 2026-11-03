//! Scene edit commands shared by the editor and the CLI.
//!
//! Every authoring operation is a [`Command`]. [`apply`] validates it
//! against the scene, performs it and returns a [`Change`]: the before and
//! after state of every entity it touched. A change can be reverted and
//! re-applied exactly ([`Change::revert`], [`Change::reapply`]), which is
//! what undo/redo ([`crate::History`]) uses.
//!
//! A command that fails leaves the scene untouched.
//!
//! Commands never reorder entities they do not touch, and new entities are
//! appended. That invariant is what makes change replay exact.

use glam::{Quat, Vec3};
use psx_core::EntityId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::components::{AudioSourceComponent, BehaviorComponent, ColliderComponent};
use crate::scene::{
    CameraStart, Entity, MAX_DEPTH, MaterialOverride, MeshComponent, Scene, SceneLighting,
    SceneSettings,
};
use crate::transform::{Transform, euler_deg_to_quat};

fn default_true() -> bool {
    true
}

/// One authoring operation. In JSON the variant is selected by `"op"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Adds an entity at the end of the list. `id` may be chosen by the
    /// caller (it must be unused); otherwise a new one is generated and
    /// reported in [`Change::created`].
    CreateEntity {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<EntityId>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<EntityId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transform: Option<Transform>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<MeshComponent>,
    },
    /// Removes an entity and all its descendants.
    DeleteEntity {
        id: EntityId,
    },
    /// Copies an entity and its descendants with fresh IDs. Parent links
    /// inside the copy are remapped to the new IDs; the copy's root keeps
    /// the original's parent. `new_id` optionally fixes the root's new ID.
    DuplicateEntity {
        id: EntityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_id: Option<EntityId>,
    },
    Rename {
        id: EntityId,
        name: String,
    },
    /// Sets any of the local transform parts. Give `rotation` (unit
    /// quaternion `[x, y, z, w]`) or `rotation_euler_deg` (`[x, y, z]`, see
    /// [`euler_deg_to_quat`]), not both.
    SetTransform {
        id: EntityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        translation: Option<Vec3>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation: Option<Quat>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation_euler_deg: Option<Vec3>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<Vec3>,
    },
    /// Moves an entity under `parent` (`None` = make it a root). With
    /// `keep_world_transform` (the default) the local transform is
    /// recomputed so the entity does not move in the world; shear that a
    /// local transform cannot express is dropped. Otherwise the local
    /// transform is kept and the entity moves with its new parent.
    SetParent {
        id: EntityId,
        parent: Option<EntityId>,
        #[serde(default = "default_true")]
        keep_world_transform: bool,
    },
    /// Adds, replaces or (with `null`) removes the mesh component.
    SetMesh {
        id: EntityId,
        mesh: Option<MeshComponent>,
    },
    /// Replaces the material override of an entity's mesh component.
    SetMaterial {
        id: EntityId,
        material: MaterialOverride,
    },
    /// Adds, replaces or (with `null`) removes the collider/body.
    SetCollider {
        id: EntityId,
        collider: Option<ColliderComponent>,
    },
    /// Adds, replaces or (with `null`) removes the audio source.
    SetAudio {
        id: EntityId,
        audio: Option<AudioSourceComponent>,
    },
    /// Replaces the entity's whole behaviour list (`[]` removes all).
    SetBehaviors {
        id: EntityId,
        behaviors: Vec<BehaviorComponent>,
    },
    /// Replaces any of the scene settings.
    SetSettings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<psx_core::RetroStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lighting: Option<SceneLighting>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<CameraStart>,
    },
}

impl Command {
    /// The `"op"` name, for reports.
    pub fn op(&self) -> &'static str {
        match self {
            Command::CreateEntity { .. } => "create_entity",
            Command::DeleteEntity { .. } => "delete_entity",
            Command::DuplicateEntity { .. } => "duplicate_entity",
            Command::Rename { .. } => "rename",
            Command::SetTransform { .. } => "set_transform",
            Command::SetParent { .. } => "set_parent",
            Command::SetMesh { .. } => "set_mesh",
            Command::SetMaterial { .. } => "set_material",
            Command::SetCollider { .. } => "set_collider",
            Command::SetAudio { .. } => "set_audio",
            Command::SetBehaviors { .. } => "set_behaviors",
            Command::SetSettings { .. } => "set_settings",
        }
    }

    /// Every op with a one-line description, for `psx schema`.
    pub const OPS: [(&'static str, &'static str); 12] = [
        (
            "create_entity",
            "{id?, name, parent?, transform?, mesh?} -> created id",
        ),
        (
            "delete_entity",
            "{id} deletes the entity and its descendants",
        ),
        (
            "duplicate_entity",
            "{id, new_id?} copies the subtree with fresh ids -> created ids",
        ),
        ("rename", "{id, name}"),
        (
            "set_transform",
            "{id, translation?, rotation? | rotation_euler_deg?, scale?}",
        ),
        (
            "set_parent",
            "{id, parent (null = root), keep_world_transform = true}",
        ),
        ("set_mesh", "{id, mesh (null removes)}"),
        ("set_material", "{id, material}"),
        (
            "set_collider",
            "{id, collider (null removes): {shape, offset?, body?, sensor?, layer?, mask?, mass?, friction?}}",
        ),
        (
            "set_audio",
            "{id, audio (null removes): {sound, bus?, volume_db?, looping?, autoplay?, spatial?, min_distance?, max_distance?}}",
        ),
        (
            "set_behaviors",
            "{id, behaviors: [{kind, params?}]} replaces the list",
        ),
        ("set_settings", "{style?, lighting?, camera?}"),
    ];
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum CommandError {
    #[error("entity {0} does not exist")]
    NoSuchEntity(EntityId),
    #[error("entity id {0} is already in use")]
    IdInUse(EntityId),
    #[error("cannot parent {child} under {parent}: it would create a cycle")]
    Cycle { child: EntityId, parent: EntityId },
    #[error("hierarchy would be deeper than {MAX_DEPTH} levels")]
    TooDeep,
    #[error("entity {0} has no mesh component")]
    NoMesh(EntityId),
    #[error("give either `rotation` or `rotation_euler_deg`, not both")]
    AmbiguousRotation,
    #[error("invalid value: {0}")]
    Invalid(String),
}

/// One entity's state before and after a change, with its list index.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityChange {
    pub id: EntityId,
    pub before: Option<(usize, Entity)>,
    pub after: Option<(usize, Entity)>,
}

/// Everything a command changed. Reverting restores the scene exactly.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Change {
    pub entities: Vec<EntityChange>,
    pub settings: Option<Box<(SceneSettings, SceneSettings)>>,
    /// IDs of entities created by the command, root first.
    pub created: Vec<EntityId>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.settings.is_none()
            && self
                .entities
                .iter()
                .all(|c| c.before.as_ref().map(|b| &b.1) == c.after.as_ref().map(|a| &a.1))
    }

    /// Restores the state before the change.
    pub fn revert(&self, scene: &mut Scene) {
        let states: Vec<_> = self.entities.iter().map(|c| (c.id, &c.before)).collect();
        replace_states(scene, &states);
        if let Some(s) = &self.settings {
            scene.settings = s.0.clone();
        }
    }

    /// Applies the change again after [`Change::revert`].
    pub fn reapply(&self, scene: &mut Scene) {
        let states: Vec<_> = self.entities.iter().map(|c| (c.id, &c.after)).collect();
        replace_states(scene, &states);
        if let Some(s) = &self.settings {
            scene.settings = s.1.clone();
        }
    }

    /// Combines `self` followed by `next` into one change, if both touch
    /// exactly the same entities (used to merge a drag into one undo step).
    pub fn merge(&self, next: &Change) -> Option<Change> {
        let same_ids = self.entities.len() == next.entities.len()
            && self
                .entities
                .iter()
                .zip(&next.entities)
                .all(|(a, b)| a.id == b.id);
        if !same_ids || self.settings.is_some() != next.settings.is_some() {
            return None;
        }
        Some(Change {
            entities: self
                .entities
                .iter()
                .zip(&next.entities)
                .map(|(a, b)| EntityChange {
                    id: a.id,
                    before: a.before.clone(),
                    after: b.after.clone(),
                })
                .collect(),
            settings: match (&self.settings, &next.settings) {
                (Some(a), Some(b)) => Some(Box::new((a.0.clone(), b.1.clone()))),
                _ => None,
            },
            created: self.created.clone(),
        })
    }
}

/// Removes every entity listed, then inserts the given states at their
/// recorded indices in ascending order. Because commands never reorder
/// other entities, this reproduces the recorded list exactly.
fn replace_states(scene: &mut Scene, states: &[(EntityId, &Option<(usize, Entity)>)]) {
    scene
        .entities
        .retain(|e| !states.iter().any(|(id, _)| *id == e.id));
    let mut inserts: Vec<&(usize, Entity)> =
        states.iter().filter_map(|(_, s)| s.as_ref()).collect();
    inserts.sort_by_key(|(i, _)| *i);
    for (index, entity) in inserts {
        let index = (*index).min(scene.entities.len());
        scene.entities.insert(index, entity.clone());
    }
}

/// Records before-states for `ids`, runs `edit`, records after-states.
/// If `edit` fails, or leaves the scene invalid, the scene is restored.
fn tracked(
    scene: &mut Scene,
    ids: &[EntityId],
    edit: impl FnOnce(&mut Scene) -> Result<(), CommandError>,
) -> Result<Change, CommandError> {
    let snapshot =
        |scene: &Scene, id: EntityId| scene.index_of(id).map(|i| (i, scene.entities[i].clone()));
    let before: Vec<_> = ids.iter().map(|&id| (id, snapshot(scene, id))).collect();
    let mut change = Change {
        entities: Vec::with_capacity(ids.len()),
        ..Default::default()
    };
    let result = edit(scene);
    for (id, b) in before {
        change.entities.push(EntityChange {
            id,
            before: b,
            after: snapshot(scene, id),
        });
    }
    let result = result.and_then(|()| check_entities(scene, ids));
    if let Err(err) = result {
        change.revert(scene);
        return Err(err);
    }
    Ok(change)
}

/// Validates the touched entities after an edit.
fn check_entities(scene: &Scene, ids: &[EntityId]) -> Result<(), CommandError> {
    for &id in ids {
        let Some(e) = scene.get(id) else { continue };
        if let Some(p) = e.local_problems().into_iter().next() {
            return Err(CommandError::Invalid(p));
        }
        if let Some(parent) = e.parent {
            if scene.get(parent).is_none() {
                return Err(CommandError::NoSuchEntity(parent));
            }
            if scene.world_matrix(id).is_none() {
                return Err(CommandError::TooDeep);
            }
        }
    }
    Ok(())
}

fn require(scene: &Scene, id: EntityId) -> Result<(), CommandError> {
    scene
        .get(id)
        .map(|_| ())
        .ok_or(CommandError::NoSuchEntity(id))
}

/// Validates and applies `command`. On error the scene is unchanged.
pub fn apply(scene: &mut Scene, command: &Command) -> Result<Change, CommandError> {
    match command {
        Command::CreateEntity {
            id,
            name,
            parent,
            transform,
            mesh,
        } => {
            let id = id.unwrap_or_default();
            if scene.get(id).is_some() {
                return Err(CommandError::IdInUse(id));
            }
            if let Some(p) = parent {
                require(scene, *p)?;
            }
            let entity = Entity {
                id,
                name: name.clone(),
                parent: *parent,
                transform: transform.unwrap_or_default(),
                mesh: mesh.clone(),
                ..Entity::new("")
            };
            let mut change = tracked(scene, &[id], |s| {
                s.entities.push(entity);
                Ok(())
            })?;
            change.created.push(id);
            Ok(change)
        }
        Command::DeleteEntity { id } => {
            require(scene, *id)?;
            let doomed = scene.subtree(*id);
            tracked(scene, &doomed, |s| {
                s.entities.retain(|e| !doomed.contains(&e.id));
                Ok(())
            })
        }
        Command::DuplicateEntity { id, new_id } => {
            require(scene, *id)?;
            let originals = scene.subtree(*id);
            let mut map = std::collections::HashMap::new();
            for (i, &old) in originals.iter().enumerate() {
                let fresh = match (i, new_id) {
                    (0, Some(n)) => *n,
                    _ => EntityId::new(),
                };
                if scene.get(fresh).is_some() {
                    return Err(CommandError::IdInUse(fresh));
                }
                map.insert(old, fresh);
            }
            let copies: Vec<Entity> = originals
                .iter()
                .filter_map(|old| scene.get(*old))
                .map(|e| {
                    let mut copy = Entity {
                        id: map[&e.id],
                        // The root keeps its parent; inner links are remapped.
                        parent: e.parent.map(|p| map.get(&p).copied().unwrap_or(p)),
                        ..e.clone()
                    };
                    // References inside the copied subtree follow the copy.
                    for b in &mut copy.behaviors {
                        b.remap_entities(&map);
                    }
                    copy
                })
                .collect();
            let new_ids: Vec<EntityId> = copies.iter().map(|e| e.id).collect();
            let mut change = tracked(scene, &new_ids, |s| {
                s.entities.extend(copies);
                Ok(())
            })?;
            change.created = new_ids;
            Ok(change)
        }
        Command::Rename { id, name } => {
            require(scene, *id)?;
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.name = name.clone();
                }
                Ok(())
            })
        }
        Command::SetTransform {
            id,
            translation,
            rotation,
            rotation_euler_deg,
            scale,
        } => {
            require(scene, *id)?;
            let rotation = match (rotation, rotation_euler_deg) {
                (Some(_), Some(_)) => return Err(CommandError::AmbiguousRotation),
                (Some(q), None) => Some(*q),
                (None, Some(deg)) => Some(euler_deg_to_quat(*deg)),
                (None, None) => None,
            };
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    let t = &mut e.transform;
                    t.translation = translation.unwrap_or(t.translation);
                    t.rotation = rotation.unwrap_or(t.rotation);
                    t.scale = scale.unwrap_or(t.scale);
                }
                Ok(())
            })
        }
        Command::SetParent {
            id,
            parent,
            keep_world_transform,
        } => {
            require(scene, *id)?;
            if let Some(p) = parent {
                require(scene, *p)?;
                if scene.is_ancestor_or_self(*id, *p) {
                    return Err(CommandError::Cycle {
                        child: *id,
                        parent: *p,
                    });
                }
            }
            let local = if *keep_world_transform {
                let world = scene.world_matrix(*id).ok_or(CommandError::TooDeep)?;
                let parent_world = match parent {
                    Some(p) => scene.world_matrix(*p).ok_or(CommandError::TooDeep)?,
                    None => glam::Mat4::IDENTITY,
                };
                Some(Transform::from_matrix(parent_world.inverse() * world))
            } else {
                None
            };
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.parent = *parent;
                    if let Some(local) = local {
                        e.transform = local;
                    }
                }
                Ok(())
            })
        }
        Command::SetMesh { id, mesh } => {
            require(scene, *id)?;
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.mesh = mesh.clone();
                }
                Ok(())
            })
        }
        Command::SetMaterial { id, material } => {
            require(scene, *id)?;
            if scene.get(*id).is_some_and(|e| e.mesh.is_none()) {
                return Err(CommandError::NoMesh(*id));
            }
            tracked(scene, &[*id], |s| {
                if let Some(m) = s.get_mut(*id).and_then(|e| e.mesh.as_mut()) {
                    m.material = *material;
                }
                Ok(())
            })
        }
        Command::SetCollider { id, collider } => {
            require(scene, *id)?;
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.collider = *collider;
                }
                Ok(())
            })
        }
        Command::SetAudio { id, audio } => {
            require(scene, *id)?;
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.audio = *audio;
                }
                Ok(())
            })
        }
        Command::SetBehaviors { id, behaviors } => {
            require(scene, *id)?;
            tracked(scene, &[*id], |s| {
                if let Some(e) = s.get_mut(*id) {
                    e.behaviors = behaviors.clone();
                }
                Ok(())
            })
        }
        Command::SetSettings {
            style,
            lighting,
            camera,
        } => {
            let before = scene.settings.clone();
            let mut after = before.clone();
            if let Some(style) = style {
                style
                    .validate()
                    .map_err(|e| CommandError::Invalid(format!("style: {e}")))?;
                after.style = style.clone();
            }
            if let Some(l) = lighting {
                after.lighting = *l;
            }
            if let Some(c) = camera {
                after.camera = *c;
            }
            // Reuse the scene-level checks for lighting and camera.
            let probe = Scene {
                settings: after.clone(),
                entities: Vec::new(),
                ..scene.clone_header()
            };
            if let Some(p) = probe.problems().first() {
                return Err(CommandError::Invalid(p.message.clone()));
            }
            scene.settings = after.clone();
            Ok(Change {
                settings: Some(Box::new((before, after))),
                ..Default::default()
            })
        }
    }
}

impl Scene {
    /// Copy of the scene without entities (cheap probe for settings checks).
    fn clone_header(&self) -> Scene {
        Scene {
            id: self.id,
            name: self.name.clone(),
            revision: self.revision,
            settings: self.settings.clone(),
            entities: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MeshSource, PrimitiveShape};

    fn create(scene: &mut Scene, name: &str, parent: Option<EntityId>, x: f32) -> EntityId {
        let change = apply(
            scene,
            &Command::CreateEntity {
                id: None,
                name: name.into(),
                parent,
                transform: Some(Transform::from_translation(Vec3::new(x, 0.0, 0.0))),
                mesh: Some(MeshComponent {
                    source: MeshSource::Primitive {
                        shape: PrimitiveShape::Cube,
                        texture: None,
                    },
                    material: MaterialOverride::default(),
                }),
            },
        )
        .unwrap();
        change.created[0]
    }

    #[test]
    fn json_form_is_tagged_and_strict() {
        let id = EntityId::new();
        let text = format!(r#"{{"op":"set_transform","id":"{id}","rotation_euler_deg":[0,90,0]}}"#);
        let cmd: Command = serde_json::from_str(&text).unwrap();
        assert_eq!(cmd.op(), "set_transform");
        let bad = format!(r#"{{"op":"rename","id":"{id}","name":"a","nmae":"b"}}"#);
        assert!(serde_json::from_str::<Command>(&bad).is_err());
        let parent: Command = serde_json::from_str(&format!(
            r#"{{"op":"set_parent","id":"{id}","parent":null}}"#
        ))
        .unwrap();
        assert!(matches!(
            parent,
            Command::SetParent {
                keep_world_transform: true,
                parent: None,
                ..
            }
        ));
        assert_eq!(Command::OPS.len(), 12);
    }

    #[test]
    fn reparent_keeps_world_position_and_rejects_cycles() {
        let mut scene = Scene::new("t");
        let a = create(&mut scene, "a", None, 5.0);
        let b = create(&mut scene, "b", None, 2.0);
        apply(
            &mut scene,
            &Command::SetTransform {
                id: a,
                translation: None,
                rotation: None,
                rotation_euler_deg: Some(Vec3::new(0.0, 90.0, 0.0)),
                scale: Some(Vec3::splat(2.0)),
            },
        )
        .unwrap();
        let world_before = scene.world_matrix(b).unwrap();
        apply(
            &mut scene,
            &Command::SetParent {
                id: b,
                parent: Some(a),
                keep_world_transform: true,
            },
        )
        .unwrap();
        assert!(
            scene
                .world_matrix(b)
                .unwrap()
                .abs_diff_eq(world_before, 1e-4)
        );
        assert!((scene.get(b).unwrap().transform.scale - Vec3::splat(0.5)).length() < 1e-5);

        let err = apply(
            &mut scene,
            &Command::SetParent {
                id: a,
                parent: Some(b),
                keep_world_transform: true,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::Cycle { .. }));
        let err = apply(
            &mut scene,
            &Command::SetParent {
                id: a,
                parent: Some(a),
                keep_world_transform: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::Cycle { .. }));

        // Without keep_world the local transform stays and the entity moves.
        apply(
            &mut scene,
            &Command::SetParent {
                id: b,
                parent: None,
                keep_world_transform: false,
            },
        )
        .unwrap();
        assert!((scene.get(b).unwrap().transform.scale - Vec3::splat(0.5)).length() < 1e-5);
    }

    #[test]
    fn failed_commands_leave_scene_unchanged() {
        let mut scene = Scene::new("t");
        let a = create(&mut scene, "a", None, 0.0);
        let before = scene.clone();
        let bad = [
            Command::SetTransform {
                id: a,
                translation: Some(Vec3::new(f32::NAN, 0.0, 0.0)),
                rotation: None,
                rotation_euler_deg: None,
                scale: None,
            },
            Command::SetTransform {
                id: a,
                translation: None,
                rotation: None,
                rotation_euler_deg: None,
                scale: Some(Vec3::ZERO),
            },
            Command::SetTransform {
                id: a,
                translation: None,
                rotation: Some(Quat::IDENTITY),
                rotation_euler_deg: Some(Vec3::ZERO),
                scale: None,
            },
            Command::DeleteEntity {
                id: EntityId::new(),
            },
            Command::CreateEntity {
                id: Some(a),
                name: "dup".into(),
                parent: None,
                transform: None,
                mesh: None,
            },
            Command::CreateEntity {
                id: None,
                name: "x".into(),
                parent: Some(EntityId::new()),
                transform: None,
                mesh: None,
            },
            Command::Rename {
                id: a,
                name: "x".repeat(crate::scene::MAX_NAME_LEN + 1),
            },
            Command::SetSettings {
                style: None,
                lighting: Some(SceneLighting {
                    direction: Vec3::ZERO,
                    ..Default::default()
                }),
                camera: None,
            },
        ];
        for cmd in &bad {
            assert!(apply(&mut scene, cmd).is_err(), "{cmd:?} should fail");
            assert_eq!(scene, before, "{cmd:?} changed the scene");
        }
    }

    #[test]
    fn delete_and_duplicate_subtrees_revert_exactly() {
        let mut scene = Scene::new("t");
        let a = create(&mut scene, "a", None, 0.0);
        let b = create(&mut scene, "b", Some(a), 1.0);
        let other = create(&mut scene, "other", None, 3.0);
        let c = create(&mut scene, "c", Some(b), 2.0);
        let original = scene.clone();

        let dup = apply(
            &mut scene,
            &Command::DuplicateEntity {
                id: a,
                new_id: None,
            },
        )
        .unwrap();
        assert_eq!(dup.created.len(), 3);
        let [ra, rb, rc] = [dup.created[0], dup.created[1], dup.created[2]];
        assert_eq!(scene.get(ra).unwrap().parent, None);
        assert_eq!(scene.get(rb).unwrap().parent, Some(ra));
        assert_eq!(scene.get(rc).unwrap().parent, Some(rb));
        assert_eq!(scene.get(rc).unwrap().name, "c");
        dup.revert(&mut scene);
        assert_eq!(scene, original);

        let del = apply(&mut scene, &Command::DeleteEntity { id: a }).unwrap();
        assert_eq!(scene.entities.len(), 1);
        assert_eq!(scene.entities[0].id, other);
        del.revert(&mut scene);
        assert_eq!(scene, original);
        del.reapply(&mut scene);
        assert!(scene.get(c).is_none());
    }

    #[test]
    fn gameplay_components_are_validated_and_duplicates_remap_references() {
        use crate::components::*;
        let mut scene = Scene::new("t");
        let door = create(&mut scene, "door", None, 0.0);
        let lever = create(&mut scene, "lever", Some(door), 1.0);

        // A moving body must be a root.
        let dynamic =
            ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }, BodyKind::Dynamic);
        let before = scene.clone();
        let err = apply(
            &mut scene,
            &Command::SetCollider {
                id: lever,
                collider: Some(dynamic),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("root"), "{err}");
        assert_eq!(scene, before);
        apply(
            &mut scene,
            &Command::SetCollider {
                id: door,
                collider: Some(ColliderComponent::new(
                    ColliderShape::Box { size: Vec3::ONE },
                    BodyKind::Kinematic,
                )),
            },
        )
        .unwrap();
        // ...and cannot be reparented under something.
        let other = create(&mut scene, "other", None, 5.0);
        assert!(
            apply(
                &mut scene,
                &Command::SetParent {
                    id: door,
                    parent: Some(other),
                    keep_world_transform: true,
                },
            )
            .is_err()
        );

        apply(
            &mut scene,
            &Command::SetBehaviors {
                id: lever,
                behaviors: vec![BehaviorComponent::new("switch").with("target", door.to_string())],
            },
        )
        .unwrap();
        let dup = apply(
            &mut scene,
            &Command::DuplicateEntity {
                id: door,
                new_id: None,
            },
        )
        .unwrap();
        let (new_door, new_lever) = (dup.created[0], dup.created[1]);
        let target = scene.get(new_lever).unwrap().behaviors[0].params["target"].clone();
        assert_eq!(target, serde_json::json!(new_door.to_string()));
        // The original still points at the original.
        let target = scene.get(lever).unwrap().behaviors[0].params["target"].clone();
        assert_eq!(target, serde_json::json!(door.to_string()));
        assert!(scene.dangling_entity_refs().is_empty());
        apply(&mut scene, &Command::DeleteEntity { id: new_door }).unwrap();
        apply(&mut scene, &Command::DeleteEntity { id: other }).unwrap();
        assert!(scene.problems().is_empty());
    }
}
