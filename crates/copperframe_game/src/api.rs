//! The game-facing API: behaviours, the game trait, and the registry that
//! connects behaviour kinds named in scene files to compiled Rust code.
//!
//! A game project registers its behaviour kinds (with parameter
//! descriptors) and optionally implements [`Game`] for global logic. The
//! engine instantiates a behaviour for each `{kind, params}` entry in the
//! scene when the scene loads or an entity is spawned.

use std::collections::BTreeMap;

use copperframe_core::EntityId;
use serde::Serialize;
use serde_json::Value;

use crate::sim::Ctx;

/// Runtime handle of an entity (valid until it is despawned). Persistent
/// [`EntityId`]s map to handles through [`Ctx::find`].
pub type Entity = hecs::Entity;

/// Per-entity gameplay code. Every method has a default that does nothing.
///
/// Calls happen on the simulation thread in this order each tick: `start`
/// (once, the tick after the entity appears), `fixed_update`, then after
/// physics `on_trigger`, and `on_interact`/`on_message` when delivered.
/// Changes requested through [`Ctx`] that create or remove entities are
/// deferred to the end of the current phase.
pub trait Behavior: 'static {
    fn start(&mut self, _ctx: &mut Ctx, _me: Entity) {}
    fn fixed_update(&mut self, _ctx: &mut Ctx, _me: Entity) {}
    /// A sensor on `me` started (`entered`) or stopped overlapping `other`.
    fn on_trigger(&mut self, _ctx: &mut Ctx, _me: Entity, _other: Entity, _entered: bool) {}
    /// The player used the interact action while aiming at `me` (or a
    /// child of `me`) and [`Behavior::interaction_prompt`] returned a prompt.
    fn on_interact(&mut self, _ctx: &mut Ctx, _me: Entity, _by: Entity) {}
    /// Text shown while the player aims at this entity ("Open door").
    /// `None` means the entity is not interactive right now.
    fn interaction_prompt(&self) -> Option<String> {
        None
    }
    /// A message sent with [`Ctx::send`].
    fn on_message(&mut self, _ctx: &mut Ctx, _me: Entity, _message: &str, _from: Option<Entity>) {}
    /// The entity is being despawned or its scene unloaded.
    fn on_destroy(&mut self, _ctx: &mut Ctx, _me: Entity) {}
}

/// Global game logic. All methods are optional.
pub trait Game: 'static {
    /// Shown on the title screen and in the window title.
    fn title(&self) -> String;
    fn register(&self, registry: &mut Registry);
    /// After each scene load.
    fn scene_started(&mut self, _ctx: &mut Ctx) {}
    /// Every fixed tick, before behaviours.
    fn fixed_update(&mut self, _ctx: &mut Ctx) {}
    /// When the game is closing.
    fn shutdown(&mut self, _ctx: &mut Ctx) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Bool,
    Number,
    Text,
    /// An entity ID (string) of the same scene.
    Entity,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub kind: FieldKind,
    pub default: Value,
    pub help: &'static str,
}

impl FieldDescriptor {
    pub fn number(name: &'static str, default: f64, help: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Number,
            default: Value::from(default),
            help,
        }
    }
    pub fn bool(name: &'static str, default: bool, help: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Bool,
            default: Value::from(default),
            help,
        }
    }
    pub fn text(name: &'static str, default: &str, help: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Text,
            default: Value::from(default),
            help,
        }
    }
    /// Entity reference; empty string = none.
    pub fn entity(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Entity,
            default: Value::from(""),
            help,
        }
    }
}

pub type CreateFn = fn(&Params) -> Result<Box<dyn Behavior>, String>;

#[derive(Clone, Serialize)]
pub struct BehaviorDescriptor {
    pub kind: &'static str,
    pub description: &'static str,
    pub fields: Vec<FieldDescriptor>,
    #[serde(skip)]
    pub create: CreateFn,
}

/// Parameters after defaults are filled in and types are checked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    values: BTreeMap<String, Value>,
}

impl Params {
    pub fn number(&self, key: &str) -> f32 {
        self.values.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32
    }
    pub fn bool(&self, key: &str) -> bool {
        self.values
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    pub fn text(&self, key: &str) -> &str {
        self.values.get(key).and_then(Value::as_str).unwrap_or("")
    }
    /// The referenced entity's persistent ID, if set.
    pub fn entity(&self, key: &str) -> Option<EntityId> {
        self.text(key).parse().ok()
    }
}

/// Behaviour kinds known to this game build.
#[derive(Default, Clone)]
pub struct Registry {
    kinds: BTreeMap<&'static str, BehaviorDescriptor>,
    /// Folder paths for the editor's Add component menu, by kind.
    folders: BTreeMap<&'static str, &'static str>,
}

/// Version of the schema document [`Registry::schema`] emits.
pub const SCHEMA_VERSION: u32 = 1;

impl Registry {
    /// A registry with the engine's built-in behaviours (`psx.*`).
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        crate::builtin::register(&mut r);
        r
    }

    /// Adds a kind. A kind registered twice keeps the last registration
    /// and logs a warning.
    pub fn register(&mut self, descriptor: BehaviorDescriptor) {
        if self.kinds.contains_key(descriptor.kind) {
            tracing::warn!("behaviour kind `{}` registered twice", descriptor.kind);
        }
        self.folders.remove(descriptor.kind);
        self.kinds.insert(descriptor.kind, descriptor);
    }

    /// Like [`Registry::register`], filing the kind under `folder` in the
    /// editor's Add component menu. Separate nested folders with `/`, e.g.
    /// `"Puzzles/Doors"`. Kinds registered without a folder are listed
    /// directly under Behaviours.
    pub fn register_in(&mut self, folder: &'static str, descriptor: BehaviorDescriptor) {
        let kind = descriptor.kind;
        self.register(descriptor);
        let folder = folder.trim_matches('/');
        if folder.is_empty() {
            self.folders.remove(kind);
        } else {
            self.folders.insert(kind, folder);
        }
    }

    /// The folder `kind` was registered in, if any.
    pub fn folder(&self, kind: &str) -> Option<&'static str> {
        self.folders.get(kind).copied()
    }

    pub fn get(&self, kind: &str) -> Option<&BehaviorDescriptor> {
        self.kinds.get(kind)
    }

    pub fn kinds(&self) -> impl Iterator<Item = &BehaviorDescriptor> {
        self.kinds.values()
    }

    /// Checks parameters against the descriptor and fills defaults.
    pub fn params(&self, b: &copperframe_scene::BehaviorComponent) -> Result<Params, String> {
        let d = self
            .get(&b.kind)
            .ok_or_else(|| format!("behaviour kind `{}` is not registered by this game", b.kind))?;
        for key in b.params.keys() {
            if !d.fields.iter().any(|f| f.name == key) {
                return Err(format!("{}: unknown parameter `{key}`", b.kind));
            }
        }
        let mut values = BTreeMap::new();
        for f in &d.fields {
            let v = b.params.get(f.name).cloned().unwrap_or(f.default.clone());
            let ok = match f.kind {
                FieldKind::Bool => v.is_boolean(),
                FieldKind::Number => v.as_f64().is_some_and(f64::is_finite),
                FieldKind::Text => v.is_string(),
                FieldKind::Entity => v
                    .as_str()
                    .is_some_and(|s| s.is_empty() || s.parse::<EntityId>().is_ok()),
            };
            if !ok {
                return Err(format!(
                    "{}.{}: expected {:?}, found {v}",
                    b.kind, f.name, f.kind
                ));
            }
            values.insert(f.name.to_owned(), v);
        }
        Ok(Params { values })
    }

    pub fn create(
        &self,
        b: &copperframe_scene::BehaviorComponent,
    ) -> Result<Box<dyn Behavior>, String> {
        let params = self.params(b)?;
        let d = self.get(&b.kind).ok_or("unregistered")?;
        (d.create)(&params).map_err(|e| format!("{}: {e}", b.kind))
    }

    /// Versioned description of every behaviour kind, emitted by game
    /// executables with `--schema` and read by the editor and the CLI.
    /// A kind registered with [`Registry::register_in`] has a `folder`.
    pub fn schema(&self, game: &str) -> Value {
        let behaviors: Vec<Value> = self
            .kinds
            .values()
            .map(|d| {
                let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
                if let (Some(folder), Some(o)) = (self.folder(d.kind), v.as_object_mut()) {
                    o.insert("folder".into(), Value::from(folder));
                }
                v
            })
            .collect();
        serde_json::json!({
            "format": "psx_game_schema",
            "version": SCHEMA_VERSION,
            "game": game,
            "behaviors": behaviors,
        })
    }
}

/// Something that happened in the simulation, for logs, the editor console
/// and headless test reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GameEvent {
    SceneLoaded {
        scene: String,
    },
    Interacted {
        entity: String,
    },
    TriggerEntered {
        sensor: String,
        other: String,
    },
    TriggerExited {
        sensor: String,
        other: String,
    },
    SoundStarted {
        entity: String,
    },
    /// A non-looping clip reached its end.
    AnimationFinished {
        entity: String,
        clip: String,
    },
    Message {
        text: String,
    },
    /// Game-specific milestone (e.g. "door_opened").
    Custom {
        name: String,
        detail: String,
    },
    Finished {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimedEvent {
    pub tick: u64,
    #[serde(flatten)]
    pub event: GameEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use copperframe_scene::BehaviorComponent;

    #[test]
    fn params_are_defaulted_and_type_checked() {
        let r = Registry::with_builtins();
        let p = r.params(&BehaviorComponent::new("psx.player")).unwrap();
        assert!(p.number("speed") > 0.0);
        let bad = BehaviorComponent::new("psx.player").with("speed", "fast");
        assert!(r.params(&bad).unwrap_err().contains("speed"));
        let unknown = BehaviorComponent::new("psx.player").with("colour", 1);
        assert!(r.params(&unknown).unwrap_err().contains("colour"));
        assert!(
            r.params(&BehaviorComponent::new("door"))
                .unwrap_err()
                .contains("not registered")
        );
        let schema = r.schema("t");
        assert_eq!(schema["format"], "psx_game_schema");
        assert!(
            schema["behaviors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["kind"] == "psx.player" && b["folder"] == "Engine/Player")
        );
    }

    #[test]
    fn folders_follow_the_latest_registration() {
        let mut r = Registry::with_builtins();
        let spin = r.get("psx.spin").unwrap().clone();
        r.register_in("/Fun/Motion/", spin.clone());
        assert_eq!(r.folder("psx.spin"), Some("Fun/Motion"));
        r.register(spin);
        assert_eq!(r.folder("psx.spin"), None);
        let schema = r.schema("t");
        let b = schema["behaviors"].as_array().unwrap();
        assert!(
            b.iter()
                .any(|b| b["kind"] == "psx.spin" && b.get("folder").is_none())
        );
    }
}
