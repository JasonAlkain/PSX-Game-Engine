//! The authored scene: entities with persistent IDs, a parent hierarchy,
//! local transforms and components, plus per-scene settings.
//!
//! This is document data, edited by the editor and the CLI through
//! [`crate::command`]. Runtime state (simulation, GPU and physics handles)
//! is built from it when a scene is loaded and never written back, so
//! stopping play mode cannot change the authored scene.
//!
//! ## Hierarchy
//!
//! Each entity names at most one parent by ID. A root has no parent. The
//! world matrix is `parent_world * local`. Cycles and references to missing
//! parents are invalid. Entities are stored in a list; the list order is
//! the display order and is preserved by every edit (new entities are
//! appended).

use std::collections::{HashMap, HashSet};

use copperframe_core::{AssetId, EntityId, RetroStyle, SceneId};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::components::{
    AnimatorComponent, AudioSourceComponent, BehaviorComponent, BodyKind, ColliderComponent,
    ColliderShape, LightComponent, RigidbodyComponent, ShadowComponent,
};
use crate::transform::Transform;

/// Hierarchies deeper than this are rejected (protects recursion and
/// keeps the transform chain cheap).
pub const MAX_DEPTH: usize = 64;
/// Longest accepted entity or scene name, in bytes.
pub const MAX_NAME_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    /// Incremented on every save. File mutations (CLI batches) may require
    /// an expected revision so concurrent edits are detected, not merged.
    pub revision: u64,
    pub settings: SceneSettings,
    pub entities: Vec<Entity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSettings {
    /// Retro style used when this scene is shown.
    pub style: RetroStyle,
    #[serde(default)]
    pub lighting: SceneLighting,
    #[serde(default)]
    pub camera: CameraStart,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            style: RetroStyle::ps1(),
            lighting: SceneLighting::default(),
            camera: CameraStart::default(),
        }
    }
}

/// One directional light plus ambient, linear RGB.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SceneLighting {
    /// Direction the light travels, from the light towards the scene.
    pub direction: Vec3,
    pub color: Vec3,
    pub ambient: Vec3,
}

impl Default for SceneLighting {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: Vec3::splat(0.85),
            ambient: Vec3::splat(0.25),
        }
    }
}

/// Where the viewing camera starts when the scene opens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraStart {
    pub position: Vec3,
    /// Degrees about +Y; positive turns left.
    pub yaw_deg: f32,
    /// Degrees about the camera's +X; positive looks up.
    pub pitch_deg: f32,
    pub fov_deg: f32,
}

impl Default for CameraStart {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.0, 1.6, 5.0),
            yaw_deg: 0.0,
            pitch_deg: -5.0,
            fov_deg: 60.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub id: EntityId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<EntityId>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshComponent>,
    /// Since scene version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collider: Option<ColliderComponent>,
    /// Since scene version 5 (moving bodies; none = static).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rigidbody: Option<RigidbodyComponent>,
    /// Since scene version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioSourceComponent>,
    /// Since scene version 2. At most one of each kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub behaviors: Vec<BehaviorComponent>,
    /// Since scene version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<LightComponent>,
    /// Since scene version 3. Needs a model mesh with a skin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animator: Option<AnimatorComponent>,
    /// Since scene version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowComponent>,
    /// Since scene version 5. The order the editor lists the components in,
    /// as [component keys](Entity::component_keys). Components not listed
    /// follow in the default order. Behaviours run in the order of
    /// [`Entity::behaviors`], which the editor keeps in step with this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_order: Vec<String>,
}

impl Entity {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: EntityId::new(),
            name: name.into(),
            parent: None,
            transform: Transform::IDENTITY,
            mesh: None,
            collider: None,
            rigidbody: None,
            audio: None,
            behaviors: Vec::new(),
            light: None,
            animator: None,
            shadow: None,
            component_order: Vec::new(),
        }
    }

    /// Whether the entity has the component named by `key` (see
    /// [`Entity::component_keys`]).
    pub fn has_component(&self, key: &str) -> bool {
        match key {
            component_key::MESH => self.mesh.is_some(),
            component_key::ANIMATOR => self.animator.is_some(),
            component_key::LIGHT => self.light.is_some(),
            component_key::SHADOW => self.shadow.is_some(),
            component_key::COLLIDER => self.collider.is_some(),
            component_key::RIGIDBODY => self.rigidbody.is_some(),
            component_key::AUDIO => self.audio.is_some(),
            _ => component_key::behavior_kind(key).is_some_and(|k| self.behavior(k).is_some()),
        }
    }

    /// Keys of the components the entity has, in the order the editor lists
    /// them: those named in `component_order` first, in that order, then the
    /// rest in the default order (built-in components, then behaviours).
    /// Keys are `mesh`, `animator`, `light`, `shadow`, `collider`, `rigidbody`, `audio`
    /// and `behavior:<kind>`.
    pub fn component_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = component_key::BUILT_IN
            .iter()
            .filter(|k| self.has_component(k))
            .map(|k| (*k).to_owned())
            .chain(
                self.behaviors
                    .iter()
                    .map(|b| component_key::behavior(&b.kind)),
            )
            .collect();
        let rank = |k: &String| {
            self.component_order
                .iter()
                .position(|o| o == k)
                .unwrap_or(usize::MAX)
        };
        // Stable: unlisted components keep the default order at the end.
        keys.sort_by_key(rank);
        keys
    }

    /// Sets the listing order to `order` (keys of components the entity
    /// has) and sorts the behaviours to match it.
    pub fn set_component_order(&mut self, order: Vec<String>) {
        self.component_order = order;
        self.prune_component_order();
        let rank = |kind: &str| {
            let key = component_key::behavior(kind);
            self.component_order
                .iter()
                .position(|o| *o == key)
                .unwrap_or(usize::MAX)
        };
        let mut behaviors = std::mem::take(&mut self.behaviors);
        behaviors.sort_by_key(|b| rank(&b.kind));
        self.behaviors = behaviors;
    }

    /// Drops order entries for components the entity no longer has, and
    /// repeated entries, so a removed component that is added again goes to
    /// the end of the list.
    pub fn prune_component_order(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        let order = std::mem::take(&mut self.component_order);
        for k in order {
            if self.has_component(&k) && !seen.contains(&k) {
                seen.push(k);
            }
        }
        self.component_order = seen;
    }

    /// How physics treats this entity's body (static without a rigidbody).
    pub fn body_kind(&self) -> BodyKind {
        BodyKind::of(self.rigidbody.as_ref())
    }

    pub fn behavior(&self, kind: &str) -> Option<&BehaviorComponent> {
        self.behaviors.iter().find(|b| b.kind == kind)
    }

    /// Problems that can be seen from the entity alone plus whether it is a
    /// root (moving bodies must be roots).
    pub fn local_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.len() > MAX_NAME_LEN {
            out.push(format!("name is longer than {MAX_NAME_LEN} bytes"));
        }
        if let Some(p) = self.transform.problem() {
            out.push(format!("transform {p}"));
        }
        if let Some(mesh) = &self.mesh
            && let Some(p) = mesh.material.problem()
        {
            out.push(format!("material: {p}"));
        }
        if let Some(MeshComponent {
            source:
                MeshSource::Primitive {
                    tile_size: Some(size),
                    ..
                },
            ..
        }) = &self.mesh
            && !(MIN_TILE_SIZE..=MAX_TILE_SIZE).contains(size)
        {
            out.push(format!(
                "tile_size must be between {MIN_TILE_SIZE} and {MAX_TILE_SIZE} metres"
            ));
        }
        if let Some(c) = &self.collider {
            if let Some(p) = c.problem() {
                out.push(format!("collider: {p}"));
            }
            if c.shape == ColliderShape::Mesh && self.mesh.is_none() {
                out.push("collider: a mesh collider needs a mesh component".into());
            }
            if c.shape == ColliderShape::Mesh && self.rigidbody.is_some() {
                out.push(
                    "collider: mesh colliders are static only; remove the rigidbody or use a \
                     box, sphere or capsule"
                        .into(),
                );
            }
        }
        if let Some(rb) = &self.rigidbody {
            if let Some(p) = rb.problem() {
                out.push(format!("rigidbody: {p}"));
            }
            if self.parent.is_some() {
                out.push(format!(
                    "rigidbody: {} bodies must be root entities (no parent)",
                    rb.kind().label().to_lowercase()
                ));
            }
        }
        if let Some(a) = &self.audio
            && let Some(p) = a.problem()
        {
            out.push(format!("audio: {p}"));
        }
        if let Some(p) = self.light.as_ref().and_then(LightComponent::problem) {
            out.push(format!("light: {p}"));
        }
        if let Some(a) = &self.animator {
            if let Some(p) = a.problem() {
                out.push(format!("animator: {p}"));
            }
            if !matches!(
                self.mesh.as_ref().map(|m| &m.source),
                Some(MeshSource::Model { .. })
            ) {
                out.push("animator: needs a mesh component showing a model asset".into());
            }
        }
        if let Some(p) = self.shadow.as_ref().and_then(ShadowComponent::problem) {
            out.push(format!("shadow: {p}"));
        }
        for (i, k) in self.component_order.iter().enumerate() {
            if !self.has_component(k) {
                out.push(format!(
                    "component_order lists `{k}`, which the entity does not have"
                ));
            } else if self.component_order[..i].contains(k) {
                out.push(format!("component_order lists `{k}` twice"));
            }
        }
        for (i, b) in self.behaviors.iter().enumerate() {
            if let Some(p) = b.problem() {
                out.push(format!("behaviour: {p}"));
            }
            if self.behaviors[..i].iter().any(|o| o.kind == b.kind) {
                out.push(format!("behaviour `{}` is attached twice", b.kind));
            }
        }
        out
    }
}

/// Names of an entity's components in [`Entity::component_order`].
pub mod component_key {
    pub const MESH: &str = "mesh";
    pub const ANIMATOR: &str = "animator";
    pub const LIGHT: &str = "light";
    pub const SHADOW: &str = "shadow";
    pub const COLLIDER: &str = "collider";
    pub const RIGIDBODY: &str = "rigidbody";
    pub const AUDIO: &str = "audio";
    /// The built-in components in their default order.
    pub const BUILT_IN: [&str; 7] = [MESH, ANIMATOR, LIGHT, SHADOW, COLLIDER, RIGIDBODY, AUDIO];
    const BEHAVIOR_PREFIX: &str = "behavior:";

    /// The key of the behaviour of `kind`.
    pub fn behavior(kind: &str) -> String {
        format!("{BEHAVIOR_PREFIX}{kind}")
    }

    /// The behaviour kind a key names, if it names one.
    pub fn behavior_kind(key: &str) -> Option<&str> {
        key.strip_prefix(BEHAVIOR_PREFIX)
    }
}

/// Draws a mesh at the entity's world transform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshComponent {
    pub source: MeshSource,
    /// Changes applied on top of the source's own materials.
    #[serde(default, skip_serializing_if = "MaterialOverride::is_empty")]
    pub material: MaterialOverride,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeshSource {
    /// A built-in unit shape, optionally textured with a texture asset
    /// (a checkerboard otherwise).
    Primitive {
        shape: PrimitiveShape,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        texture: Option<AssetId>,
        /// Metres one repeat of the texture covers on the entity's faces, so scaling
        /// the entity shows more of the texture rather than stretching it. Without
        /// it the texture covers each face once. Spheres ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tile_size: Option<f32>,
    },
    /// Every mesh of an imported model asset (glTF/GLB).
    Model { asset: AssetId },
}

impl MeshSource {
    /// The assets this source needs.
    pub fn asset_refs(&self) -> impl Iterator<Item = AssetId> {
        match self {
            MeshSource::Primitive { texture, .. } => *texture,
            MeshSource::Model { asset } => Some(*asset),
        }
        .into_iter()
    }
}

/// Built-in shapes, all centred on the origin with unit size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveShape {
    /// 1×1×1 cube.
    Cube,
    /// 1×1 floor tile in XZ facing +Y, 4×4 quads.
    Plane,
    /// Sphere of diameter 1.
    Sphere,
    /// 1×1 quad in XY facing +Z.
    Quad,
}

/// Tile size new primitives start with: a unit cube looks as it always did, and
/// scaling it up repeats the texture.
pub const DEFAULT_TILE_SIZE: f32 = 1.0;
pub const MIN_TILE_SIZE: f32 = 0.01;
pub const MAX_TILE_SIZE: f32 = 1000.0;

impl PrimitiveShape {
    /// Whether a primitive's `tile_size` applies: flat faces only.
    pub fn tiles(self) -> bool {
        self != PrimitiveShape::Sphere
    }

    pub const ALL: [PrimitiveShape; 4] = [
        PrimitiveShape::Cube,
        PrimitiveShape::Plane,
        PrimitiveShape::Sphere,
        PrimitiveShape::Quad,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PrimitiveShape::Cube => "Cube",
            PrimitiveShape::Plane => "Plane",
            PrimitiveShape::Sphere => "Sphere",
            PrimitiveShape::Quad => "Quad",
        }
    }
}

/// Per-entity material changes. Each field that is set replaces the value
/// from the source material; unset fields keep it. `tint` multiplies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MaterialOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<AlphaSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flat_shading: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_affine: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_sided: Option<bool>,
}

impl MaterialOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn problem(&self) -> Option<String> {
        if let Some(tint) = self.tint
            && !tint.iter().all(|c| c.is_finite() && *c >= 0.0)
        {
            return Some("tint must be finite and not negative".to_owned());
        }
        if let Some(AlphaSetting::Cutout { threshold }) = self.alpha
            && !(0.0..=1.0).contains(&threshold)
        {
            return Some(format!("cutout threshold {threshold} is outside 0..=1"));
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlphaSetting {
    Opaque,
    Cutout { threshold: f32 },
    Blend,
}

/// One reason a scene is invalid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SceneProblem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityId>,
    pub message: String,
}

impl std::fmt::Display for SceneProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.entity {
            Some(id) => write!(f, "entity {id}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: SceneId::new(),
            name: name.into(),
            revision: 0,
            settings: SceneSettings::default(),
            entities: Vec::new(),
        }
    }

    pub fn index_of(&self, id: EntityId) -> Option<usize> {
        self.entities.iter().position(|e| e.id == id)
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// Direct children of `parent` (`None` = roots), in list order.
    pub fn children(&self, parent: Option<EntityId>) -> impl Iterator<Item = &Entity> {
        self.entities.iter().filter(move |e| e.parent == parent)
    }

    /// `root` followed by all its descendants, depth first in list order.
    /// Entities reachable only through a cycle are not returned.
    pub fn subtree(&self, root: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            out.push(id);
            let kids: Vec<EntityId> = self.children(Some(id)).map(|e| e.id).collect();
            stack.extend(kids.into_iter().rev());
        }
        out
    }

    /// True when `ancestor` is `id` itself or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: EntityId, id: EntityId) -> bool {
        let mut current = Some(id);
        for _ in 0..=self.entities.len() {
            match current {
                Some(c) if c == ancestor => return true,
                Some(c) => current = self.get(c).and_then(|e| e.parent),
                None => return false,
            }
        }
        // A cycle that does not include `ancestor`.
        false
    }

    /// World matrix of `id`, or `None` if it does not exist or its parent
    /// chain is broken (missing parent, cycle, too deep).
    pub fn world_matrix(&self, id: EntityId) -> Option<Mat4> {
        let mut m = Mat4::IDENTITY;
        let mut current = Some(id);
        let mut depth = 0;
        while let Some(c) = current {
            let e = self.get(c)?;
            m = e.transform.matrix() * m;
            current = e.parent;
            depth += 1;
            if depth > MAX_DEPTH {
                return None;
            }
        }
        Some(m)
    }

    /// World matrices of every entity whose parent chain is valid.
    pub fn world_matrices(&self) -> HashMap<EntityId, Mat4> {
        let index: HashMap<EntityId, &Entity> = self.entities.iter().map(|e| (e.id, e)).collect();
        let mut out: HashMap<EntityId, Mat4> = HashMap::with_capacity(self.entities.len());
        for e in &self.entities {
            // Walk up to the first ancestor already computed (or a root).
            let mut chain = vec![e];
            let mut base = Mat4::IDENTITY;
            let mut ok = true;
            loop {
                let last = chain[chain.len() - 1];
                let Some(p) = last.parent else { break };
                if let Some(m) = out.get(&p) {
                    base = *m;
                    break;
                }
                match index.get(&p) {
                    Some(parent) if chain.len() <= MAX_DEPTH => chain.push(parent),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            for link in chain.into_iter().rev() {
                base *= link.transform.matrix();
                out.insert(link.id, base);
            }
        }
        out
    }

    /// Every problem that makes the scene unusable. Asset references are
    /// not checked here (that needs the project's asset database).
    pub fn problems(&self) -> Vec<SceneProblem> {
        let mut problems = Vec::new();
        let mut push = |entity: Option<EntityId>, message: String| {
            problems.push(SceneProblem { entity, message })
        };
        if self.name.len() > MAX_NAME_LEN {
            push(
                None,
                format!("scene name is longer than {MAX_NAME_LEN} bytes"),
            );
        }
        if let Err(err) = self.settings.style.validate() {
            push(None, format!("style: {err}"));
        }
        let l = &self.settings.lighting;
        if !l
            .direction
            .to_array()
            .into_iter()
            .chain(l.color.to_array())
            .chain(l.ambient.to_array())
            .all(f32::is_finite)
            || l.direction.length_squared() < 1e-12
        {
            push(
                None,
                "lighting must be finite with a non-zero direction".to_owned(),
            );
        }
        let c = &self.settings.camera;
        if !c.position.is_finite() || !c.yaw_deg.is_finite() || !c.pitch_deg.is_finite() {
            push(None, "camera start must be finite".to_owned());
        }
        if !(1.0..=170.0).contains(&c.fov_deg) {
            push(
                None,
                format!("camera field of view {} is outside 1..=170", c.fov_deg),
            );
        }

        let mut ids = HashSet::new();
        for e in &self.entities {
            if !ids.insert(e.id) {
                push(Some(e.id), "duplicate entity id".to_owned());
            }
        }
        for e in &self.entities {
            let id = Some(e.id);
            for p in e.local_problems() {
                push(id, p);
            }
            match e.parent {
                Some(p) if p == e.id => push(id, "is its own parent".to_owned()),
                Some(p) if !ids.contains(&p) => push(id, format!("parent {p} does not exist")),
                _ => {}
            }
        }
        // Cycles and depth: walk each chain with a step limit.
        for e in &self.entities {
            let mut current = e.parent;
            let mut depth = 0;
            while let Some(c) = current {
                if c == e.id {
                    push(Some(e.id), "is part of a parent cycle".to_owned());
                    break;
                }
                depth += 1;
                if depth > MAX_DEPTH {
                    push(
                        Some(e.id),
                        format!("hierarchy is deeper than {MAX_DEPTH} levels"),
                    );
                    break;
                }
                current = self.get(c).and_then(|p| p.parent);
            }
        }
        problems
    }

    /// Every asset ID referenced by the scene, with the entities using it.
    pub fn asset_refs(&self) -> Vec<(EntityId, AssetId)> {
        let mut out: Vec<(EntityId, AssetId)> = self
            .entities
            .iter()
            .filter_map(|e| e.mesh.as_ref().map(|m| (e.id, m)))
            .flat_map(|(id, m)| m.source.asset_refs().map(move |a| (id, a)))
            .collect();
        out.extend(
            self.entities
                .iter()
                .filter_map(|e| e.audio.as_ref().map(|a| (e.id, a.sound))),
        );
        out
    }

    /// Behaviour parameters that name an entity which does not exist:
    /// `(entity, "kind.param", missing id)`.
    pub fn dangling_entity_refs(&self) -> Vec<(EntityId, String, EntityId)> {
        let ids: HashSet<EntityId> = self.entities.iter().map(|e| e.id).collect();
        let mut out = Vec::new();
        for e in &self.entities {
            for b in &e.behaviors {
                for (key, target) in b.entity_refs() {
                    if !ids.contains(&target) {
                        out.push((e.id, format!("{}.{key}", b.kind), target));
                    }
                }
            }
        }
        out
    }

    /// The first entity with a behaviour of `kind`.
    pub fn find_behavior(&self, kind: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.behavior(kind).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::euler_deg_to_quat;

    fn child(name: &str, parent: &Entity, t: Vec3) -> Entity {
        let mut e = Entity::new(name);
        e.parent = Some(parent.id);
        e.transform.translation = t;
        e
    }

    #[test]
    fn world_matrix_composes_parent_first() {
        let mut scene = Scene::new("t");
        let mut root = Entity::new("root");
        root.transform.translation = Vec3::new(10.0, 0.0, 0.0);
        root.transform.rotation = euler_deg_to_quat(Vec3::new(0.0, 90.0, 0.0));
        root.transform.scale = Vec3::splat(2.0);
        let kid = child("kid", &root, Vec3::new(0.0, 0.0, -1.0));
        let grandkid = child("gk", &kid, Vec3::new(0.0, 1.0, 0.0));
        scene.entities = vec![grandkid.clone(), root.clone(), kid.clone()];
        // -Z under a 90° yaw points to -X, scaled by 2.
        let p = scene
            .world_matrix(kid.id)
            .unwrap()
            .transform_point3(Vec3::ZERO);
        assert!(p.abs_diff_eq(Vec3::new(8.0, 0.0, 0.0), 1e-5), "{p:?}");
        let all = scene.world_matrices();
        assert_eq!(all.len(), 3);
        let g = all[&grandkid.id].transform_point3(Vec3::ZERO);
        assert!(g.abs_diff_eq(Vec3::new(8.0, 2.0, 0.0), 1e-5), "{g:?}");
        assert!(scene.problems().is_empty());
        assert_eq!(scene.subtree(root.id), vec![root.id, kid.id, grandkid.id]);
    }

    #[test]
    fn cycles_and_missing_parents_are_problems() {
        let mut scene = Scene::new("t");
        let mut a = Entity::new("a");
        let mut b = Entity::new("b");
        a.parent = Some(b.id);
        b.parent = Some(a.id);
        let mut orphan = Entity::new("orphan");
        orphan.parent = Some(EntityId::new());
        scene.entities = vec![a.clone(), b.clone(), orphan.clone()];
        let problems = scene.problems();
        assert!(
            problems
                .iter()
                .any(|p| p.entity == Some(a.id) && p.message.contains("cycle"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.entity == Some(orphan.id) && p.message.contains("does not exist"))
        );
        assert_eq!(scene.world_matrix(a.id), None);
        assert!(!scene.world_matrices().contains_key(&a.id));
        // Does not loop forever.
        assert!(!scene.is_ancestor_or_self(orphan.id, a.id));
        assert_eq!(scene.subtree(a.id).len(), 2);
    }

    #[test]
    fn duplicate_ids_and_bad_values_are_problems() {
        let mut scene = Scene::new("t");
        let mut e = Entity::new("e");
        e.mesh = Some(MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Cube,
                texture: None,
                tile_size: None,
            },
            material: MaterialOverride {
                alpha: Some(AlphaSetting::Cutout { threshold: 2.0 }),
                ..Default::default()
            },
        });
        scene.entities = vec![e.clone(), e.clone()];
        let text: Vec<String> = scene.problems().iter().map(|p| p.to_string()).collect();
        assert!(text.iter().any(|t| t.contains("duplicate")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("threshold")), "{text:?}");
        scene.settings.camera.fov_deg = 0.0;
        assert!(
            scene
                .problems()
                .iter()
                .any(|p| p.message.contains("field of view"))
        );
    }
}
