//! Gameplay components: colliders and bodies, audio sources and behaviours
//! (scene format version 2); local lights, animators and blob shadows
//! (version 3).
//!
//! These are authored data only. The runtime turns them into physics
//! bodies, audio emitters and behaviour instances when a scene is loaded;
//! handles never appear here.

use std::collections::BTreeMap;

use copperframe_core::{AssetId, EntityId};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Number of collision layers (`layer` is `0..LAYER_COUNT`, `mask` a bit
/// per layer).
pub const LAYER_COUNT: u8 = 16;
/// Longest accepted behaviour kind name, in bytes.
pub const MAX_KIND_LEN: usize = 64;
/// Prefix reserved for behaviours provided by the engine.
pub const ENGINE_BEHAVIOR_PREFIX: &str = "psx.";

/// A collision shape. Without a [`RigidbodyComponent`] on the same entity
/// the collider is static (it never moves); with one it moves with that
/// body.
///
/// Sizes are in the entity's local units and are scaled by the entity's
/// world scale when the body is created: boxes scale per axis; spheres use
/// the largest axis; capsules use the largest horizontal axis for the
/// radius and Y for the height. Rotation and translation come from the
/// entity's world transform.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColliderComponent {
    pub shape: ColliderShape,
    /// Shape centre relative to the entity origin (local units).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: Vec3,
    /// Sensors report overlaps (trigger events) but never push anything.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sensor: bool,
    /// Collision layer, `0..16`.
    #[serde(default, skip_serializing_if = "is_default_layer")]
    pub layer: u8,
    /// Layers this collider interacts with, one bit per layer.
    #[serde(default = "all_layers", skip_serializing_if = "is_all_layers")]
    pub mask: u16,
    /// Friction coefficient, `0..=10`.
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub friction: f32,
    /// Bounciness, `0..=1`: 0 stops dead, 1 bounces back at full speed.
    /// Since scene version 5.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub restitution: f32,
}

fn is_zero(v: &Vec3) -> bool {
    *v == Vec3::ZERO
}
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}
fn is_default_layer(v: &u8) -> bool {
    *v == 0
}
fn all_layers() -> u16 {
    u16::MAX
}
fn is_all_layers(v: &u16) -> bool {
    *v == u16::MAX
}
fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn half() -> f32 {
    0.5
}
fn is_half(v: &f32) -> bool {
    *v == 0.5
}
fn default_angular_damping() -> f32 {
    DEFAULT_ANGULAR_DAMPING
}
fn is_default_angular_damping(v: &f32) -> bool {
    *v == DEFAULT_ANGULAR_DAMPING
}

impl ColliderComponent {
    pub fn new(shape: ColliderShape) -> Self {
        Self {
            shape,
            offset: Vec3::ZERO,
            sensor: false,
            layer: 0,
            mask: u16::MAX,
            friction: 0.5,
            restitution: 0.0,
        }
    }

    pub fn problem(&self) -> Option<String> {
        let positive = |v: f32| v.is_finite() && v > 0.0;
        match self.shape {
            ColliderShape::Box { size } if !size.to_array().into_iter().all(positive) => {
                return Some("box size must be positive and finite".into());
            }
            ColliderShape::Sphere { radius } if !positive(radius) => {
                return Some("sphere radius must be positive and finite".into());
            }
            ColliderShape::Capsule { radius, height } if !positive(radius) || !positive(height) => {
                return Some("capsule radius and height must be positive and finite".into());
            }
            _ => {}
        }
        if !self.offset.is_finite() {
            return Some("collider offset must be finite".into());
        }
        if self.layer >= LAYER_COUNT {
            return Some(format!(
                "collision layer {} is outside 0..{LAYER_COUNT}",
                self.layer
            ));
        }
        if !(self.friction.is_finite() && (0.0..=10.0).contains(&self.friction)) {
            return Some(format!("friction {} is outside 0..=10", self.friction));
        }
        if !(self.restitution.is_finite() && (0.0..=1.0).contains(&self.restitution)) {
            return Some(format!("bounciness {} is outside 0..=1", self.restitution));
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ColliderShape {
    /// Box with full edge lengths `size`.
    Box {
        size: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// Vertical capsule: `height` is the total height including the caps.
    Capsule {
        radius: f32,
        height: f32,
    },
    /// The triangles of the entity's own mesh (static colliders only).
    Mesh,
}

impl ColliderShape {
    pub fn label(&self) -> &'static str {
        match self {
            ColliderShape::Box { .. } => "Box",
            ColliderShape::Sphere { .. } => "Sphere",
            ColliderShape::Capsule { .. } => "Capsule",
            ColliderShape::Mesh => "Mesh",
        }
    }
}

/// Default [`RigidbodyComponent::angular_damping`]: a little, so rolling
/// objects come to rest.
pub const DEFAULT_ANGULAR_DAMPING: f32 = 0.05;
/// Largest accepted damping value.
pub const MAX_DAMPING: f32 = 100.0;
/// Largest accepted gravity scale magnitude.
pub const MAX_GRAVITY_SCALE: f32 = 100.0;

/// Makes the entity a moving physics body (since scene version 5; earlier
/// versions stored the body kind on the collider). Must be a root entity.
///
/// - Dynamic (the default): gravity, forces and contacts move it, and its
///   pose is written back to the entity every tick.
/// - Kinematic: moved by gameplay code (doors, lifts) through its
///   transform; pushes dynamic bodies but is never pushed.
///
/// Without a collider a dynamic body is a point mass: it falls but does not
/// touch anything.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidbodyComponent {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kinematic: bool,
    /// Mass in kilograms (dynamic bodies).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub mass: f32,
    /// Multiplies the project's gravity for this body (0 floats, -1 falls
    /// upwards). Dynamic bodies only.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub gravity_scale: f32,
    /// Slows linear motion over time (air drag), `0..=100`.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub linear_damping: f32,
    /// Slows rotation over time, `0..=100`.
    #[serde(
        default = "default_angular_damping",
        skip_serializing_if = "is_default_angular_damping"
    )]
    pub angular_damping: f32,
    /// Keeps the body upright: contacts never rotate it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lock_rotation: bool,
}

impl Default for RigidbodyComponent {
    fn default() -> Self {
        Self::dynamic()
    }
}

impl RigidbodyComponent {
    pub fn dynamic() -> Self {
        Self {
            kinematic: false,
            mass: 1.0,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: DEFAULT_ANGULAR_DAMPING,
            lock_rotation: false,
        }
    }

    pub fn kinematic() -> Self {
        Self {
            kinematic: true,
            ..Self::dynamic()
        }
    }

    pub fn kind(&self) -> BodyKind {
        if self.kinematic {
            BodyKind::Kinematic
        } else {
            BodyKind::Dynamic
        }
    }

    pub fn problem(&self) -> Option<String> {
        if !(self.mass.is_finite() && self.mass > 0.0) {
            return Some("mass must be positive and finite".into());
        }
        if !(self.gravity_scale.is_finite() && self.gravity_scale.abs() <= MAX_GRAVITY_SCALE) {
            return Some(format!(
                "gravity scale {} is outside -{MAX_GRAVITY_SCALE}..={MAX_GRAVITY_SCALE}",
                self.gravity_scale
            ));
        }
        for (name, v) in [
            ("linear damping", self.linear_damping),
            ("angular damping", self.angular_damping),
        ] {
            if !(v.is_finite() && (0.0..=MAX_DAMPING).contains(&v)) {
                return Some(format!("{name} {v} is outside 0..={MAX_DAMPING}"));
            }
        }
        None
    }
}

/// How the physics simulation treats a body: derived from whether the
/// entity has a [`RigidbodyComponent`] and whether it is kinematic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// Never moves; placed from the entity's world transform at load.
    #[default]
    Static,
    /// Simulated: gravity and contacts move it.
    Dynamic,
    /// Moved by gameplay code; pushes dynamic bodies but is not pushed.
    Kinematic,
}

impl BodyKind {
    pub const ALL: [BodyKind; 3] = [BodyKind::Static, BodyKind::Dynamic, BodyKind::Kinematic];

    pub fn of(rigidbody: Option<&RigidbodyComponent>) -> Self {
        rigidbody.map_or(BodyKind::Static, RigidbodyComponent::kind)
    }

    pub fn label(self) -> &'static str {
        match self {
            BodyKind::Static => "Static",
            BodyKind::Dynamic => "Dynamic",
            BodyKind::Kinematic => "Kinematic",
        }
    }
}

/// Mixer bus a sound plays on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBus {
    #[default]
    Effects,
    Music,
}

/// A sound emitted from the entity's world position (or played flat when
/// `spatial` is off).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSourceComponent {
    /// An audio asset (WAV or Ogg Vorbis).
    pub sound: AssetId,
    #[serde(default)]
    pub bus: AudioBus,
    /// Gain in decibels, `-60..=12`.
    #[serde(default)]
    pub volume_db: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub looping: bool,
    /// Starts when the scene starts. Otherwise gameplay code plays it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub autoplay: bool,
    #[serde(default = "yes")]
    pub spatial: bool,
    /// Full volume within this distance (meters).
    #[serde(default = "one")]
    pub min_distance: f32,
    /// Silent beyond this distance (meters).
    #[serde(default = "twenty")]
    pub max_distance: f32,
}

fn yes() -> bool {
    true
}
fn twenty() -> f32 {
    20.0
}

impl AudioSourceComponent {
    pub fn new(sound: AssetId) -> Self {
        Self {
            sound,
            bus: AudioBus::Effects,
            volume_db: 0.0,
            looping: false,
            autoplay: false,
            spatial: true,
            min_distance: 1.0,
            max_distance: 20.0,
        }
    }

    pub fn problem(&self) -> Option<String> {
        if !(self.volume_db.is_finite() && (-60.0..=12.0).contains(&self.volume_db)) {
            return Some(format!("volume {} dB is outside -60..=12", self.volume_db));
        }
        if !(self.min_distance.is_finite()
            && self.max_distance.is_finite()
            && self.min_distance > 0.0
            && self.max_distance > self.min_distance)
        {
            return Some("audio distances must satisfy 0 < min_distance < max_distance".into());
        }
        None
    }
}

/// A point light at the entity's world position. Its effect fades to zero
/// at `range`, so a light only touches what is near it. How many lights a
/// view uses, and whether they are lit per vertex or per pixel, is a style
/// setting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightComponent {
    /// Linear RGB, `0..=1` per channel.
    #[serde(default = "white")]
    pub color: Vec3,
    /// Multiplies `color`, `0..=16`.
    #[serde(default = "one")]
    pub intensity: f32,
    /// Meters, `0.05..=100`.
    #[serde(default = "four")]
    pub range: f32,
}

fn white() -> Vec3 {
    Vec3::ONE
}
fn four() -> f32 {
    4.0
}

impl Default for LightComponent {
    fn default() -> Self {
        Self {
            color: Vec3::ONE,
            intensity: 1.0,
            range: 4.0,
        }
    }
}

impl LightComponent {
    pub fn problem(&self) -> Option<String> {
        if !self
            .color
            .to_array()
            .iter()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
        {
            return Some("light colour channels must be within 0..=1".into());
        }
        if !(self.intensity.is_finite() && (0.0..=16.0).contains(&self.intensity)) {
            return Some(format!(
                "light intensity {} is outside 0..=16",
                self.intensity
            ));
        }
        if !(self.range.is_finite() && (0.05..=100.0).contains(&self.range)) {
            return Some(format!("light range {} is outside 0.05..=100", self.range));
        }
        None
    }
}

/// Longest accepted animation clip name, in bytes.
pub const MAX_CLIP_NAME_LEN: usize = 128;

/// Plays an animation clip of the entity's skinned model. Gameplay code
/// can switch clips at run time (with a crossfade); this is the clip the
/// entity starts with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnimatorComponent {
    /// Clip name as it appears in the model (`copperframe inspect` lists them).
    pub clip: String,
    /// Playback rate; 1 is normal speed, `-10..=10`.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f32,
    #[serde(default = "yes")]
    pub looping: bool,
    /// Start playing when the scene starts; otherwise the model holds the
    /// clip's first frame until gameplay code plays something.
    #[serde(default = "yes")]
    pub autoplay: bool,
}

impl AnimatorComponent {
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            speed: 1.0,
            looping: true,
            autoplay: true,
        }
    }

    pub fn problem(&self) -> Option<String> {
        if self.clip.is_empty() || self.clip.len() > MAX_CLIP_NAME_LEN {
            return Some(format!(
                "animator clip name must be 1..={MAX_CLIP_NAME_LEN} bytes"
            ));
        }
        if !(self.speed.is_finite() && (-10.0..=10.0).contains(&self.speed)) {
            return Some(format!("animator speed {} is outside -10..=10", self.speed));
        }
        None
    }
}

/// A blob shadow: a soft dark disc on lit, upward-facing surfaces below
/// the entity. Cheap, and readable at PS1 resolutions; whether blob shadows
/// are drawn, and how dark, is a style setting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowComponent {
    /// Disc radius in local units (scaled by the entity's largest
    /// horizontal world scale), `0.05..=20`.
    #[serde(default = "half")]
    pub radius: f32,
    /// Bottom of the caster relative to the entity origin (local units,
    /// not rotated). Shadows fall on surfaces below this point.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: Vec3,
}

impl Default for ShadowComponent {
    fn default() -> Self {
        Self {
            radius: 0.5,
            offset: Vec3::ZERO,
        }
    }
}

impl ShadowComponent {
    pub fn problem(&self) -> Option<String> {
        if !(self.radius.is_finite() && (0.05..=20.0).contains(&self.radius)) {
            return Some(format!(
                "shadow radius {} is outside 0.05..=20",
                self.radius
            ));
        }
        if !self.offset.is_finite() || self.offset.abs().max_element() > 1000.0 {
            return Some("shadow offset must be finite and within 1000 units".into());
        }
        None
    }
}

/// A behaviour attached to an entity: a registered kind name plus
/// parameters. Kinds are registered by the engine (`psx.*`) or by the game
/// project; the game's schema (`copperframe schema`, the editor's inspector)
/// describes each kind's parameters. Parameter values are JSON scalars;
/// entity references are entity IDs written as strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorComponent {
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

impl BehaviorComponent {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            params: BTreeMap::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }

    pub fn problem(&self) -> Option<String> {
        let valid_kind = !self.kind.is_empty()
            && self.kind.len() <= MAX_KIND_LEN
            && self
                .kind
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
        if !valid_kind {
            return Some(format!(
                "behaviour kind `{}` must be 1..={MAX_KIND_LEN} characters of a-z, 0-9, `_` or `.`",
                self.kind
            ));
        }
        for (key, value) in &self.params {
            if key.is_empty() || key.len() > MAX_KIND_LEN {
                return Some(format!(
                    "{}: parameter names must be 1..=64 bytes",
                    self.kind
                ));
            }
            match value {
                serde_json::Value::Bool(_) | serde_json::Value::String(_) => {}
                serde_json::Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => {}
                _ => {
                    return Some(format!(
                        "{}.{key}: parameters must be a boolean, finite number or string",
                        self.kind
                    ));
                }
            }
        }
        None
    }

    /// Entity IDs mentioned by string parameters.
    pub fn entity_refs(&self) -> impl Iterator<Item = (&str, EntityId)> {
        self.params.iter().filter_map(|(k, v)| {
            v.as_str()
                .and_then(|s| s.parse::<EntityId>().ok())
                .map(|id| (k.as_str(), id))
        })
    }

    /// Rewrites string parameters that name an entity in `map`.
    pub fn remap_entities(&mut self, map: &std::collections::HashMap<EntityId, EntityId>) {
        for value in self.params.values_mut() {
            if let Some(new) = value
                .as_str()
                .and_then(|s| s.parse::<EntityId>().ok())
                .and_then(|id| map.get(&id))
            {
                *value = serde_json::Value::String(new.to_string());
            }
        }
    }

    pub fn param_f32(&self, key: &str) -> Option<f32> {
        self.params
            .get(key)
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_omitted_and_restored() {
        let c = ColliderComponent::new(ColliderShape::Box {
            size: Vec3::new(1.0, 2.0, 3.0),
        });
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(text, r#"{"shape":{"kind":"box","size":[1.0,2.0,3.0]}}"#);
        assert_eq!(serde_json::from_str::<ColliderComponent>(&text).unwrap(), c);
        assert!(c.problem().is_none());
        let rb = RigidbodyComponent::dynamic();
        assert_eq!(serde_json::to_string(&rb).unwrap(), "{}");
        assert_eq!(
            serde_json::from_str::<RigidbodyComponent>("{}").unwrap(),
            rb
        );
        let k = RigidbodyComponent::kinematic();
        assert_eq!(serde_json::to_string(&k).unwrap(), r#"{"kinematic":true}"#);
        assert_eq!(k.kind(), BodyKind::Kinematic);
        assert_eq!(BodyKind::of(None), BodyKind::Static);
    }

    #[test]
    fn invalid_components_are_described() {
        let mut c = ColliderComponent::new(ColliderShape::Mesh);
        c.layer = 16;
        assert!(c.problem().unwrap().contains("layer"));
        c.layer = 0;
        c.restitution = 1.5;
        assert!(c.problem().unwrap().contains("bounciness"));
        let mut rb = RigidbodyComponent::dynamic();
        rb.mass = 0.0;
        assert!(rb.problem().unwrap().contains("mass"));
        rb.mass = 2.0;
        rb.linear_damping = -1.0;
        assert!(rb.problem().unwrap().contains("linear damping"));
        let mut a = AudioSourceComponent::new(AssetId::new());
        a.max_distance = 0.5;
        assert!(a.problem().is_some());
        let b = BehaviorComponent::new("door").with("speed", f64::NAN);
        // NaN cannot be represented in JSON: serde_json stores Null.
        assert!(b.problem().unwrap().contains("speed"));
        assert!(BehaviorComponent::new("Bad Kind!").problem().is_some());
    }

    #[test]
    fn entity_params_are_remapped() {
        let old = EntityId::new();
        let new = EntityId::new();
        let mut b = BehaviorComponent::new("switch")
            .with("target", old.to_string())
            .with("label", "Lever");
        assert_eq!(b.entity_refs().collect::<Vec<_>>(), vec![("target", old)]);
        b.remap_entities(&[(old, new)].into_iter().collect());
        assert_eq!(b.params["target"], serde_json::json!(new.to_string()));
        assert_eq!(b.params["label"], serde_json::json!("Lever"));
    }
}
