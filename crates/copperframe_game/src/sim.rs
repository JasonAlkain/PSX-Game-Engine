//! The simulation: a runtime world built from an authored scene, advanced
//! in fixed ticks. No window, GPU or audio device is required, so the same
//! code runs in the standalone game, editor play mode (a child process) and
//! headless tests.
//!
//! ## Tick order
//!
//! 1. Behaviours that appeared since the last tick get `start`.
//! 2. Game `fixed_update`, then every behaviour's `fixed_update`.
//! 3. Interaction: the player's aim ray picks the interactive entity; the
//!    interact action delivers `on_interact`.
//! 4. Deferred commands (spawn, despawn, messages) are applied.
//! 5. World transforms are recomputed; kinematic bodies receive targets.
//! 6. Physics steps; dynamic bodies write their poses back.
//! 7. Trigger events are delivered (`on_trigger`), then contact events
//!    (`on_collision`), then commands again.
//! 8. Animators advance (finished non-looping clips are logged), then
//!    transforms, audio emitters and the listener are updated.
//!
//! An entity spawned during a phase is visible to queries after that
//! phase; it gets `start` on the next tick. A despawned entity disappears
//! at the end of the phase that requested it (with its descendants).
//!
//! ## Authored data
//!
//! The simulation reads a [`Scene`] and never writes it. Stopping play mode
//! therefore cannot change the authored scene.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use copperframe_assets::project::{AssetDatabase, ImportedAsset, Project};
use copperframe_assets::{AssetKind, ImportedModel};
use copperframe_core::{AssetId, EntityId};
use copperframe_scene::{
    BodyKind, ColliderShape, Entity as SceneEntity, LightComponent, MeshComponent, MeshSource,
    Scene, ShadowComponent, Transform,
};
use glam::{Mat4, Quat, Vec3};
use serde::Serialize;
use thiserror::Error;

use crate::actions::{ActionState, names};
use crate::animation::Animator;
use crate::api::{Behavior, Entity, Game, GameEvent, Registry, ScriptDiagnostic, TimedEvent};
use crate::audio::Audio;
use crate::physics::{BodyHandle, CharacterBody, MeshTriangles, Physics, QueryShape, RayHit};

/// Fixed simulation rate.
pub const TICK_HZ: u32 = 60;
pub const TICK_DT: f32 = 1.0 / TICK_HZ as f32;

// ---------------------------------------------------------------------------
// Engine components stored in the hecs world.

/// Persistent ID from the scene (or assigned at spawn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Persistent(pub EntityId);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name(pub String);
/// Local transform relative to the parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Local(pub Transform);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);
/// World matrix this tick and last tick (for render interpolation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldMatrix {
    pub current: Mat4,
    pub previous: Mat4,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh(pub MeshComponent);
/// Hidden entities are not drawn (their colliders stay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hidden;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body(pub BodyHandle);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Character(pub CharacterBody);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emitter;
/// The authored scene entity an instance was created from (spawn copies).
#[derive(Debug, Clone, PartialEq)]
pub struct Source(pub Arc<SceneEntity>);

// ---------------------------------------------------------------------------

/// What the active camera sees, written by the player (or a camera zone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraState {
    pub position: Vec3,
    /// Radians about +Y; positive turns left.
    pub yaw: f32,
    /// Radians; positive looks up.
    pub pitch: f32,
    pub fov_deg: f32,
}

impl CameraState {
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }
    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
    pub fn from_matrix(m: Mat4, fov_deg: f32) -> Self {
        let (_, r, t) = m.to_scale_rotation_translation();
        let f = r * Vec3::NEG_Z;
        Self {
            position: t,
            yaw: (-f.x).atan2(-f.z),
            pitch: f.y.clamp(-1.0, 1.0).asin(),
            fov_deg,
        }
    }
}

/// Text overlays requested by gameplay.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Hud {
    /// Interaction prompt for this tick ("E: Open door").
    pub prompt: Option<String>,
    /// A message with the seconds it stays visible.
    pub message: Option<(String, f32)>,
}

/// Persistent game progress (saved by the player app in the user's data
/// directory, separate from project files).
pub type GameState = BTreeMap<String, serde_json::Value>;

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Finished { message: String },
    Quit,
}

enum Cmd {
    Spawn(Box<SceneEntity>),
    Despawn(Entity),
    Message {
        to: Entity,
        from: Option<Entity>,
        text: String,
    },
    Interact {
        target: Entity,
        by: Entity,
    },
    LoadScene(String),
    Finish(String),
    Quit,
}

/// Requested aim ray for interaction this tick.
#[derive(Debug, Clone, Copy)]
struct Aim {
    by: Entity,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
}

/// Everything behaviours can reach through [`Ctx`].
pub struct Core {
    pub world: hecs::World,
    ids: HashMap<EntityId, Entity>,
    physics: Physics<Entity>,
    audio: Audio<Entity>,
    pub camera: CameraState,
    /// Entity whose transform is the camera (fixed-camera zones).
    pub camera_override: Option<Entity>,
    pub hud: Hud,
    pub state: GameState,
    events: Vec<TimedEvent>,
    commands: Vec<Cmd>,
    aim: Option<Aim>,
    tick: u64,
    order: Vec<Entity>,
    order_dirty: bool,
    assets: Assets,
    scene: Option<Arc<Scene>>,
    diagnostics: Vec<ScriptDiagnostic>,
}

/// Asset access for spawning: CPU models (for mesh colliders) and sounds.
struct Assets {
    project: Project,
    db: AssetDatabase,
    models: HashMap<AssetId, Option<Arc<ImportedModel>>>,
    sounds: HashSet<AssetId>,
}

/// Access to the simulation from gameplay code.
pub struct Ctx<'a> {
    core: &'a mut Core,
    input: &'a ActionState,
}

impl Ctx<'_> {
    pub fn input(&self) -> &ActionState {
        self.input
    }
    pub fn dt(&self) -> f32 {
        TICK_DT
    }
    pub fn tick(&self) -> u64 {
        self.core.tick
    }
    pub fn world(&self) -> &hecs::World {
        &self.core.world
    }
    /// Direct ECS access for game components. Engine components (`Local`,
    /// `Body`, ...) should be changed through the methods below.
    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.core.world
    }

    pub fn find(&self, id: EntityId) -> Option<Entity> {
        self.core.ids.get(&id).copied()
    }
    pub fn find_by_name(&self, name: &str) -> Option<Entity> {
        self.core
            .world
            .query::<(Entity, &Name)>()
            .iter()
            .find(|(_, n)| n.0 == name)
            .map(|(e, _)| e)
    }
    pub fn id_of(&self, e: Entity) -> Option<EntityId> {
        self.core.world.get::<&Persistent>(e).ok().map(|p| p.0)
    }
    pub fn name(&self, e: Entity) -> String {
        entity_name(&self.core.world, e)
    }
    pub fn exists(&self, e: Entity) -> bool {
        self.core.world.contains(e)
    }

    pub fn local(&self, e: Entity) -> Option<Transform> {
        self.core.world.get::<&Local>(e).ok().map(|l| l.0)
    }
    /// Sets the local transform. For kinematic bodies this is the next
    /// physics target; dynamic bodies are teleported.
    pub fn set_local(&mut self, e: Entity, t: Transform) {
        if let Ok(mut l) = self.core.world.get::<&mut Local>(e) {
            l.0 = t;
        }
        if let Ok(b) = self.core.world.get::<&Body>(e)
            && b.0.kind == BodyKind::Dynamic
        {
            self.core.physics.teleport(b.0, t.translation, t.rotation);
        }
    }
    /// World matrix as of the last transform update.
    pub fn world_matrix(&self, e: Entity) -> Option<Mat4> {
        self.core
            .world
            .get::<&WorldMatrix>(e)
            .ok()
            .map(|w| w.current)
    }
    pub fn world_position(&self, e: Entity) -> Option<Vec3> {
        self.world_matrix(e).map(|m| m.w_axis.truncate())
    }
    pub fn parent(&self, e: Entity) -> Option<Entity> {
        self.core.world.get::<&Parent>(e).ok().map(|p| p.0)
    }
    pub fn set_hidden(&mut self, e: Entity, hidden: bool) {
        if hidden {
            let _ = self.core.world.insert_one(e, Hidden);
        } else {
            let _ = self.core.world.remove_one::<Hidden>(e);
        }
    }

    // --- physics ------------------------------------------------------

    /// Creates the character capsule used by [`Ctx::move_character`].
    pub fn add_character(&mut self, e: Entity, feet: Vec3, radius: f32, height: f32) {
        let ch = self.core.physics.add_character(e, feet, radius, height);
        let _ = self.core.world.insert_one(e, Character(ch));
    }
    /// Moves `e`'s character by `desired` meters; updates its local
    /// translation. Returns the new feet position and grounded state.
    pub fn move_character(&mut self, e: Entity, desired: Vec3) -> Option<(Vec3, bool)> {
        let ch = self.core.world.get::<&Character>(e).ok()?.0;
        let (feet, grounded) = self.core.physics.move_character(&ch, desired, TICK_DT);
        if let Ok(mut l) = self.core.world.get::<&mut Local>(e) {
            l.0.translation = feet;
        }
        Some((feet, grounded))
    }
    /// First solid collider along a ray; `ignore`'s body is skipped.
    pub fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        ignore: Option<Entity>,
    ) -> Option<RayHit<Entity>> {
        let exclude = ignore.and_then(|e| self.exclude_handle(e));
        self.core
            .physics
            .raycast(origin, direction, max_distance, exclude)
    }
    fn body(&self, e: Entity) -> Option<BodyHandle> {
        self.core.world.get::<&Body>(e).ok().map(|b| b.0)
    }

    /// Linear velocity of `e`'s rigidbody in m/s (zero without one).
    pub fn velocity(&self, e: Entity) -> Vec3 {
        self.body(e)
            .map_or(Vec3::ZERO, |b| self.core.physics.linear_velocity(b))
    }
    /// Sets the linear velocity of a dynamic rigidbody.
    pub fn set_velocity(&mut self, e: Entity, v: Vec3) {
        if let Some(b) = self.body(e) {
            self.core.physics.set_linear_velocity(b, v);
        }
    }
    /// Angular velocity in radians per second about each world axis.
    pub fn angular_velocity(&self, e: Entity) -> Vec3 {
        self.body(e)
            .map_or(Vec3::ZERO, |b| self.core.physics.angular_velocity(b))
    }
    pub fn set_angular_velocity(&mut self, e: Entity, v: Vec3) {
        if let Some(b) = self.body(e) {
            self.core.physics.set_angular_velocity(b, v);
        }
    }
    /// Kicks a dynamic rigidbody: changes its velocity by `impulse / mass`
    /// immediately (newton-seconds).
    pub fn apply_impulse(&mut self, e: Entity, impulse: Vec3) {
        if let Some(b) = self.body(e) {
            self.core.physics.apply_impulse(b, impulse);
        }
    }
    /// Spins a dynamic rigidbody immediately.
    pub fn apply_torque_impulse(&mut self, e: Entity, impulse: Vec3) {
        if let Some(b) = self.body(e) {
            self.core.physics.apply_torque_impulse(b, impulse);
        }
    }
    /// Pushes a dynamic rigidbody with `force` newtons for this tick only;
    /// call every tick (e.g. from `fixed_update`) for a steady push.
    pub fn add_force(&mut self, e: Entity, force: Vec3) {
        if let Some(b) = self.body(e) {
            self.core.physics.add_force(b, force);
        }
    }
    /// Mass of `e`'s dynamic rigidbody in kilograms (0 otherwise).
    pub fn mass(&self, e: Entity) -> f32 {
        self.body(e).map_or(0.0, |b| self.core.physics.mass(b))
    }
    /// Whether `e`'s dynamic rigidbody has come to rest.
    pub fn is_sleeping(&self, e: Entity) -> bool {
        self.body(e)
            .is_some_and(|b| self.core.physics.is_sleeping(b))
    }
    pub fn gravity(&self) -> Vec3 {
        self.core.physics.gravity()
    }
    /// Changes gravity for the rest of the session (the project setting is
    /// the starting value).
    pub fn set_gravity(&mut self, g: Vec3) {
        self.core.physics.set_gravity(g);
    }
    /// Mass the player character pushes dynamic bodies with (default
    /// [`crate::physics::CHARACTER_PUSH_MASS`] kg); 0 stops it pushing.
    pub fn set_character_push_mass(&mut self, mass: f32) {
        self.core.physics.set_character_push_mass(mass);
    }
    /// Entities whose colliders overlap a sphere. Sensors are skipped
    /// unless `include_sensors`; `ignore`'s own body is skipped.
    pub fn overlap_sphere(
        &self,
        center: Vec3,
        radius: f32,
        include_sensors: bool,
        ignore: Option<Entity>,
    ) -> Vec<Entity> {
        self.overlap(
            QueryShape::Sphere { radius },
            center,
            Quat::IDENTITY,
            include_sensors,
            ignore,
        )
    }
    /// Entities whose colliders overlap a box (`half` extents, rotated).
    pub fn overlap_box(
        &self,
        center: Vec3,
        half: Vec3,
        rotation: Quat,
        include_sensors: bool,
        ignore: Option<Entity>,
    ) -> Vec<Entity> {
        self.overlap(
            QueryShape::Box { half },
            center,
            rotation,
            include_sensors,
            ignore,
        )
    }
    /// Entities whose colliders overlap `shape`.
    pub fn overlap(
        &self,
        shape: QueryShape,
        center: Vec3,
        rotation: Quat,
        include_sensors: bool,
        ignore: Option<Entity>,
    ) -> Vec<Entity> {
        let exclude = ignore.and_then(|e| self.exclude_handle(e));
        self.core
            .physics
            .overlap(shape, center, rotation, include_sensors, exclude)
    }
    fn exclude_handle(&self, e: Entity) -> Option<BodyHandle> {
        self.core
            .world
            .get::<&Character>(e)
            .ok()
            .map(|c| c.0.handle)
            .or_else(|| self.body(e))
    }

    /// Asks the engine to resolve interaction along this ray this tick.
    pub fn aim(&mut self, by: Entity, origin: Vec3, direction: Vec3, max_distance: f32) {
        self.core.aim = Some(Aim {
            by,
            origin,
            direction,
            max_distance,
        });
    }

    // --- camera and HUD ----------------------------------------------

    pub fn camera(&self) -> CameraState {
        self.core.camera
    }
    pub fn set_camera(&mut self, c: CameraState) {
        self.core.camera = c;
    }
    pub fn set_camera_override(&mut self, e: Option<Entity>) {
        self.core.camera_override = e;
    }
    pub fn camera_override(&self) -> Option<Entity> {
        self.core.camera_override
    }
    /// Shows `text` for `seconds`.
    pub fn show_message(&mut self, text: impl Into<String>, seconds: f32) {
        let text = text.into();
        self.core.hud.message = Some((text.clone(), seconds));
        self.log(GameEvent::Message { text });
    }

    // --- audio --------------------------------------------------------

    /// Starts `e`'s audio source.
    pub fn play_sound(&mut self, e: Entity) {
        self.core.audio.play(e, 0.0);
        let entity = self.name(e);
        self.log(GameEvent::SoundStarted { entity });
    }
    pub fn stop_sound(&mut self, e: Entity, fade_secs: f32) {
        self.core.audio.stop(e, fade_secs);
    }
    pub fn is_sound_playing(&self, e: Entity) -> bool {
        self.core.audio.is_playing(e)
    }

    // --- deferred commands -------------------------------------------

    /// Plays `clip` on `e`'s animator, crossfading from the current pose
    /// over `fade_secs`. Returns `false` when `e` has no animator or the
    /// model has no such clip (nothing changes then).
    pub fn play_animation(&mut self, e: Entity, clip: &str, fade_secs: f32, looping: bool) -> bool {
        self.core
            .world
            .get::<&mut Animator>(e)
            .is_ok_and(|mut a| a.play(clip, fade_secs, 1.0, looping, true))
    }
    /// Like [`Ctx::play_animation`] with a playback speed (negative plays
    /// backwards, `-10..=10`).
    pub fn play_animation_at(
        &mut self,
        e: Entity,
        clip: &str,
        fade_secs: f32,
        looping: bool,
        speed: f32,
    ) -> bool {
        self.core
            .world
            .get::<&mut Animator>(e)
            .is_ok_and(|mut a| a.play(clip, fade_secs, speed, looping, true))
    }
    /// Fades `e` back to its model's rest pose.
    pub fn stop_animation(&mut self, e: Entity, fade_secs: f32) {
        if let Ok(mut a) = self.core.world.get::<&mut Animator>(e) {
            a.stop(fade_secs);
        }
    }
    /// The clip playing on `e` and its clip-local time in seconds.
    pub fn animation(&self, e: Entity) -> Option<(String, f32)> {
        let a = self.core.world.get::<&Animator>(e).ok()?;
        a.state().map(|(c, t)| (c.to_owned(), t))
    }
    /// `true` while `e`'s clip is running and has not reached its end.
    pub fn is_animation_playing(&self, e: Entity) -> bool {
        self.core
            .world
            .get::<&Animator>(e)
            .is_ok_and(|a| a.is_playing())
    }
    /// Clip names of `e`'s model (empty without an animator).
    pub fn animation_clips(&self, e: Entity) -> Vec<String> {
        self.core
            .world
            .get::<&Animator>(e)
            .map(|a| a.clip_names().into_iter().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Delivers `message` to every behaviour on `to` after this phase.
    pub fn send(&mut self, to: Entity, from: Option<Entity>, message: impl Into<String>) {
        self.core.commands.push(Cmd::Message {
            to,
            from,
            text: message.into(),
        });
    }
    /// Spawns an entity after this phase. A nil/duplicate ID is replaced
    /// with a fresh one; the ID used is returned.
    pub fn spawn(&mut self, mut entity: SceneEntity) -> EntityId {
        if self.core.ids.contains_key(&entity.id) {
            entity.id = EntityId::new();
        }
        let id = entity.id;
        self.core.commands.push(Cmd::Spawn(Box::new(entity)));
        id
    }
    /// Copies an authored entity (and its children) from the loaded scene
    /// with fresh IDs at `transform`; references inside the copy are
    /// remapped. Returns the new root's ID.
    pub fn instantiate(&mut self, template: EntityId, transform: Transform) -> Option<EntityId> {
        let scene = self.core.scene.clone()?;
        let ids = scene.subtree(template);
        if ids.is_empty() {
            return None;
        }
        let map: HashMap<EntityId, EntityId> =
            ids.iter().map(|&old| (old, EntityId::new())).collect();
        for (i, old) in ids.iter().enumerate() {
            let src = scene.get(*old)?;
            let mut copy = src.clone();
            copy.id = map[old];
            copy.parent = if i == 0 {
                None
            } else {
                src.parent.map(|p| map.get(&p).copied().unwrap_or(p))
            };
            if i == 0 {
                copy.transform = transform;
            }
            for b in &mut copy.behaviors {
                b.remap_entities(&map);
            }
            self.core.commands.push(Cmd::Spawn(Box::new(copy)));
        }
        Some(map[&template])
    }
    pub fn despawn(&mut self, e: Entity) {
        self.core.commands.push(Cmd::Despawn(e));
    }
    /// Unloads the current scene and loads `path` (project-relative) at the
    /// end of the tick. Game state is kept.
    pub fn load_scene(&mut self, path: impl Into<String>) {
        self.core.commands.push(Cmd::LoadScene(path.into()));
    }
    /// Ends the run successfully (the player app shows `message`).
    pub fn finish(&mut self, message: impl Into<String>) {
        self.core.commands.push(Cmd::Finish(message.into()));
    }
    pub fn quit(&mut self) {
        self.core.commands.push(Cmd::Quit);
    }

    // --- state and logging -------------------------------------------

    pub fn state(&self) -> &GameState {
        &self.core.state
    }
    pub fn state_mut(&mut self) -> &mut GameState {
        &mut self.core.state
    }
    /// Reports a script problem (the log, the editor, headless reports).
    pub fn report_script_problem(&mut self, mut d: ScriptDiagnostic) {
        d.tick.get_or_insert(self.core.tick);
        tracing::error!("{d}");
        self.core.diagnostics.push(d);
    }

    pub fn log(&mut self, event: GameEvent) {
        tracing::info!(tick = self.core.tick, "{event:?}");
        self.core.events.push(TimedEvent {
            tick: self.core.tick,
            event,
        });
    }
}

/// Translation-only interpolation between the last two ticks (rotation and
/// scale from the current tick), enough for light and shadow placement.
fn lerp_matrix(w: &WorldMatrix, alpha: f32) -> Mat4 {
    let mut m = w.current;
    let t = w
        .previous
        .w_axis
        .lerp(w.current.w_axis, alpha.clamp(0.0, 1.0));
    m.w_axis = t;
    m
}

fn entity_name(world: &hecs::World, e: Entity) -> String {
    world
        .get::<&Name>(e)
        .map(|n| n.0.clone())
        .unwrap_or_else(|_| format!("{e:?}"))
}

struct Slot {
    entity: Entity,
    kind: String,
    behavior: Option<Box<dyn Behavior>>,
    started: bool,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("could not load scene {path}: {message}")]
    Scene { path: PathBuf, message: String },
    #[error("scene {path} cannot be played:\n{}", .problems.join("\n"))]
    Invalid {
        path: PathBuf,
        problems: Vec<String>,
    },
}

/// One drawable entity for the renderer.
pub struct DrawEntity {
    pub id: EntityId,
    pub mesh: MeshComponent,
    pub current: Mat4,
    pub previous: Mat4,
    /// Joint palette of an animated model (`None`: rest pose).
    pub joints: Option<Vec<Mat4>>,
}

pub struct Simulation {
    core: Core,
    slots: Vec<Slot>,
    registry: Registry,
    game: Box<dyn Game>,
    outcome: Option<Outcome>,
    scene_path: Option<PathBuf>,
    previous_camera: CameraState,
    pending_scene: Option<String>,
    warnings: Vec<String>,
}

impl Simulation {
    /// A simulation for `project`. `audio` is the output (silent for
    /// headless runs).
    pub fn new(project: Project, game: Box<dyn Game>, audio: Audio<Entity>) -> Self {
        let mut registry = Registry::with_builtins();
        game.register(&mut registry);
        let load_problems = crate::script_api::register_project_scripts(&project, &mut registry);
        for d in &load_problems {
            tracing::error!("{d}");
        }
        let db = AssetDatabase::scan(&project);
        let camera = CameraState {
            position: Vec3::new(0.0, 1.6, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_deg: 60.0,
        };
        Self {
            core: Core {
                world: hecs::World::new(),
                ids: HashMap::new(),
                physics: {
                    let mut p = Physics::new(TICK_DT);
                    p.set_gravity(project.file.physics.gravity);
                    p
                },
                audio,
                camera,
                camera_override: None,
                hud: Hud::default(),
                state: GameState::new(),
                events: Vec::new(),
                commands: Vec::new(),
                aim: None,
                tick: 0,
                order: Vec::new(),
                order_dirty: true,
                assets: Assets {
                    project,
                    db,
                    models: HashMap::new(),
                    sounds: HashSet::new(),
                },
                scene: None,
                diagnostics: load_problems,
            },
            slots: Vec::new(),
            registry,
            game,
            outcome: None,
            scene_path: None,
            previous_camera: camera,
            pending_scene: None,
            warnings: Vec::new(),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }
    pub fn title(&self) -> String {
        self.game.title()
    }
    /// The asset database scanned when the simulation was created.
    pub fn asset_db(&self) -> &AssetDatabase {
        &self.core.assets.db
    }

    pub fn project(&self) -> &Project {
        &self.core.assets.project
    }
    pub fn scene(&self) -> Option<&Scene> {
        self.core.scene.as_deref()
    }
    pub fn scene_path(&self) -> Option<&Path> {
        self.scene_path.as_deref()
    }
    pub fn tick_count(&self) -> u64 {
        self.core.tick
    }
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }
    pub fn hud(&self) -> &Hud {
        &self.core.hud
    }
    pub fn state(&self) -> &GameState {
        &self.core.state
    }
    pub fn set_state(&mut self, state: GameState) {
        self.core.state = state;
    }
    pub fn audio_mut(&mut self) -> &mut Audio<Entity> {
        &mut self.core.audio
    }
    pub fn world(&self) -> &hecs::World {
        &self.core.world
    }
    pub fn find(&self, id: EntityId) -> Option<Entity> {
        self.core.ids.get(&id).copied()
    }
    /// The interaction prompt the behaviours on `e` offer now, if any.
    pub fn interaction_prompt(&self, e: Entity) -> Option<String> {
        self.slots
            .iter()
            .filter(|s| s.entity == e)
            .find_map(|s| s.behavior.as_ref().and_then(|b| b.interaction_prompt()))
    }
    pub fn entity_count(&self) -> usize {
        self.core.world.len() as usize
    }
    pub fn body_count(&self) -> usize {
        self.core.physics.body_count()
    }
    pub fn emitter_count(&self) -> usize {
        self.core.audio.emitter_count()
    }
    /// Cached CPU models and decoded sounds (lifecycle tests).
    pub fn cached_assets(&self) -> (usize, usize) {
        (self.core.assets.models.len(), self.core.assets.sounds.len())
    }
    /// Behaviour kinds attached to `e`.
    pub fn behaviors_of(&self, e: Entity) -> Vec<&str> {
        self.slots
            .iter()
            .filter(|s| s.entity == e)
            .map(|s| s.kind.as_str())
            .collect()
    }

    /// Script problems since the last call: scripts that did not load
    /// (reported once, at start) and callbacks that failed.
    pub fn take_diagnostics(&mut self) -> Vec<ScriptDiagnostic> {
        std::mem::take(&mut self.core.diagnostics)
    }

    /// Load-time warnings (missing assets and the like).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
    pub fn take_events(&mut self) -> Vec<TimedEvent> {
        std::mem::take(&mut self.core.events)
    }

    /// The camera to render, interpolated between the last two ticks.
    pub fn camera(&self, alpha: f32) -> CameraState {
        let c = self.current_camera();
        let p = self.previous_camera;
        let lerp_angle = |a: f32, b: f32| {
            let mut d = b - a;
            while d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            while d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            a + d * alpha
        };
        CameraState {
            position: p.position.lerp(c.position, alpha),
            yaw: lerp_angle(p.yaw, c.yaw),
            pitch: p.pitch + (c.pitch - p.pitch) * alpha,
            fov_deg: c.fov_deg,
        }
    }

    fn current_camera(&self) -> CameraState {
        match self.core.camera_override.and_then(|e| {
            self.core
                .world
                .get::<&WorldMatrix>(e)
                .ok()
                .map(|w| w.current)
        }) {
            Some(m) => CameraState::from_matrix(m, self.core.camera.fov_deg),
            None => self.core.camera,
        }
    }

    /// Visible meshes with their last two world matrices.
    pub fn draw_list(&self) -> Vec<DrawEntity> {
        self.core
            .world
            .query::<(
                &Persistent,
                &Mesh,
                &WorldMatrix,
                Option<&Animator>,
                Option<&Hidden>,
            )>()
            .iter()
            .filter(|(_, _, _, _, h)| h.is_none())
            .map(|(p, m, w, a, _)| DrawEntity {
                id: p.0,
                mesh: m.0.clone(),
                current: w.current,
                previous: w.previous,
                joints: a.map(|a| a.palette().to_vec()),
            })
            .collect()
    }

    /// Point lights of visible entities with their interpolated world
    /// matrices (`alpha` as for [`Simulation::camera`]).
    pub fn lights(&self, alpha: f32) -> Vec<(Mat4, LightComponent)> {
        self.core
            .world
            .query::<(&Source, &WorldMatrix, Option<&Hidden>)>()
            .iter()
            .filter(|(_, _, h)| h.is_none())
            .filter_map(|(s, w, _)| Some((lerp_matrix(w, alpha), s.0.light?)))
            .collect()
    }

    /// Blob shadow casters of visible entities (see [`Simulation::lights`]).
    pub fn shadows(&self, alpha: f32) -> Vec<(Mat4, ShadowComponent)> {
        self.core
            .world
            .query::<(&Source, &WorldMatrix, Option<&Hidden>)>()
            .iter()
            .filter(|(_, _, h)| h.is_none())
            .filter_map(|(s, w, _)| Some((lerp_matrix(w, alpha), s.0.shadow?)))
            .collect()
    }

    /// Loads a scene file (project-relative or absolute), replacing the
    /// current one. Behaviour kinds and parameters are checked first: a
    /// scene that cannot be played is not loaded.
    pub fn load_scene_file(&mut self, path: &Path) -> Result<(), LoadError> {
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.core.assets.project.root.join(path)
        };
        let scene = copperframe_scene::load_scene(&full).map_err(|e| LoadError::Scene {
            path: full.clone(),
            message: e.to_string(),
        })?;
        self.load_scene(scene, &full)
    }

    /// Loads an in-memory scene (editor play mode passes a temporary copy).
    pub fn load_scene(&mut self, scene: Scene, path: &Path) -> Result<(), LoadError> {
        let mut problems: Vec<String> = Vec::new();
        for e in &scene.entities {
            for b in &e.behaviors {
                if let Err(err) = self.registry.params(b) {
                    problems.push(format!("\"{}\": {err}", e.name));
                }
            }
        }
        if !problems.is_empty() {
            return Err(LoadError::Invalid {
                path: path.to_path_buf(),
                problems,
            });
        }
        self.unload();
        // Drop cached models and decoded sounds the new scene does not use,
        // so long sessions over many scenes do not grow without bound.
        let keep: HashSet<AssetId> = scene.asset_refs().into_iter().map(|(_, a)| a).collect();
        self.core.assets.models.retain(|id, _| keep.contains(id));
        self.core.assets.sounds.retain(|id| keep.contains(id));
        self.core.audio.retain_sounds(&keep);
        let scene = Arc::new(scene);
        self.core.scene = Some(scene.clone());
        self.scene_path = Some(path.to_path_buf());
        // Rescan so newly imported assets are found.
        self.core.assets.db = AssetDatabase::scan(&self.core.assets.project);
        let s = &scene.settings.camera;
        self.core.camera = CameraState {
            position: s.position,
            yaw: s.yaw_deg.to_radians(),
            pitch: s.pitch_deg.to_radians(),
            fov_deg: s.fov_deg,
        };
        // Parents before children so parent handles exist.
        let mut ordered: Vec<&SceneEntity> = Vec::with_capacity(scene.entities.len());
        let mut placed: HashSet<EntityId> = HashSet::new();
        while ordered.len() < scene.entities.len() {
            let before = ordered.len();
            for e in &scene.entities {
                if !placed.contains(&e.id) && e.parent.is_none_or(|p| placed.contains(&p)) {
                    placed.insert(e.id);
                    ordered.push(e);
                }
            }
            if ordered.len() == before {
                break; // invalid hierarchy; Scene::problems prevents this
            }
        }
        for e in ordered {
            self.spawn_now(e.clone());
        }
        self.update_transforms(true);
        self.create_bodies_and_emitters(None);
        self.core.physics.refresh();
        self.previous_camera = self.current_camera();
        let label = self.core.assets.project.relative(path);
        self.ctx_log(GameEvent::SceneLoaded { scene: label });
        let input = ActionState::default();
        let mut ctx = Ctx {
            core: &mut self.core,
            input: &input,
        };
        self.game.scene_started(&mut ctx);
        Ok(())
    }

    fn ctx_log(&mut self, event: GameEvent) {
        let input = ActionState::default();
        Ctx {
            core: &mut self.core,
            input: &input,
        }
        .log(event);
    }

    /// Despawns everything (scene end). Behaviours get `on_destroy`.
    pub fn unload(&mut self) {
        let roots: Vec<Entity> = self
            .core
            .world
            .query::<(Entity, Option<&Parent>)>()
            .iter()
            .filter(|(_, p)| p.is_none())
            .map(|(e, _)| e)
            .collect();
        for e in roots {
            self.despawn_now(e);
        }
        // Anything left (should be nothing) goes too.
        self.core.world.clear();
        self.core.ids.clear();
        self.core.audio.clear_emitters();
        self.core.camera_override = None;
        self.core.hud = Hud::default();
        self.slots.clear();
        self.core.commands.clear();
        self.core.order_dirty = true;
        self.core.scene = None;
    }

    /// Creates the hecs entity and behaviour instances (no physics/audio
    /// yet; see `create_bodies_and_emitters`).
    fn spawn_now(&mut self, e: SceneEntity) -> Option<Entity> {
        if self.core.ids.contains_key(&e.id) {
            self.warnings
                .push(format!("entity id {} already exists; spawn skipped", e.id));
            return None;
        }
        let parent = e.parent.and_then(|p| self.core.ids.get(&p).copied());
        let source = Arc::new(e.clone());
        let mut builder = hecs::EntityBuilder::new();
        builder.add(Persistent(e.id));
        builder.add(Name(e.name.clone()));
        builder.add(Local(e.transform));
        builder.add(WorldMatrix {
            current: Mat4::IDENTITY,
            previous: Mat4::IDENTITY,
        });
        builder.add(Source(source));
        if let Some(p) = parent {
            builder.add(Parent(p));
        }
        if let Some(m) = &e.mesh {
            builder.add(Mesh(m.clone()));
        }
        if let Some(animator) = self.animator_for(&e) {
            builder.add(animator);
        }
        let handle = self.core.world.spawn(builder.build());
        self.core.ids.insert(e.id, handle);
        self.core.order_dirty = true;
        for b in &e.behaviors {
            match self.registry.create(b) {
                Ok(behavior) => self.slots.push(Slot {
                    entity: handle,
                    kind: b.kind.clone(),
                    behavior: Some(behavior),
                    started: false,
                }),
                Err(err) => self.warnings.push(format!("\"{}\": {err}", e.name)),
            }
        }
        Some(handle)
    }

    /// The runtime animator for an entity's `animator` component, with its
    /// initial clip started (or held). Problems become load warnings.
    fn animator_for(&mut self, e: &SceneEntity) -> Option<Animator> {
        let a = e.animator.as_ref()?;
        let Some(MeshSource::Model { asset }) = e.mesh.as_ref().map(|m| &m.source) else {
            self.warnings
                .push(format!("\"{}\": the animator needs a model mesh", e.name));
            return None;
        };
        let Some(model) = self.core.assets.model(*asset) else {
            self.warnings.push(format!(
                "\"{}\": model {asset} could not be loaded; animator ignored",
                e.name
            ));
            return None;
        };
        let Some(mut animator) = Animator::new(model) else {
            self.warnings.push(format!(
                "\"{}\": the model has no skeleton; animator ignored",
                e.name
            ));
            return None;
        };
        if !animator.play(&a.clip, 0.0, a.speed, a.looping, a.autoplay) {
            self.warnings.push(format!(
                "\"{}\": the model has no clip \"{}\" (it has: {})",
                e.name,
                a.clip,
                animator.clip_names().join(", ")
            ));
        }
        Some(animator)
    }

    /// Physics bodies and audio emitters for entities that do not have
    /// them yet (`only`: just these entities).
    fn create_bodies_and_emitters(&mut self, only: Option<&[Entity]>) {
        let pending: Vec<(Entity, Arc<SceneEntity>)> = self
            .core
            .world
            .query::<(Entity, &Source, Option<&Body>, Option<&Emitter>)>()
            .iter()
            .filter(|(e, _, b, a)| {
                only.is_none_or(|o| o.contains(e)) && (b.is_none() || a.is_none())
            })
            .map(|(e, s, _, _)| (e, s.0.clone()))
            .collect();
        for (handle, src) in pending {
            let world = self
                .core
                .world
                .get::<&WorldMatrix>(handle)
                .map(|w| w.current)
                .unwrap_or(Mat4::IDENTITY);
            if (src.collider.is_some() || src.rigidbody.is_some())
                && self.core.world.get::<&Body>(handle).is_err()
            {
                let c = src.collider.as_ref();
                let mesh = if c.is_some_and(|c| c.shape == ColliderShape::Mesh) {
                    src.mesh
                        .as_ref()
                        .and_then(|m| self.core.assets.triangles(&m.source))
                } else {
                    None
                };
                match self
                    .core
                    .physics
                    .add(handle, c, src.rigidbody.as_ref(), world, mesh.as_ref())
                {
                    Ok(body) => {
                        let _ = self.core.world.insert_one(handle, Body(body));
                    }
                    Err(err) => self
                        .warnings
                        .push(format!("\"{}\": physics body not created: {err}", src.name)),
                }
            }
            if let Some(a) = &src.audio
                && self.core.world.get::<&Emitter>(handle).is_err()
            {
                if let Err(err) = self.core.assets.load_sound(&mut self.core.audio, a.sound) {
                    self.warnings
                        .push(format!("\"{}\": sound unavailable: {err}", src.name));
                }
                self.core
                    .audio
                    .add_emitter(handle, *a, world.w_axis.truncate());
                let _ = self.core.world.insert_one(handle, Emitter);
                if a.autoplay {
                    self.ctx_log(GameEvent::SoundStarted {
                        entity: src.name.clone(),
                    });
                }
            }
        }
    }

    fn despawn_now(&mut self, root: Entity) {
        if !self.core.world.contains(root) {
            return;
        }
        // Collect the subtree (children reference parents).
        let mut doomed = vec![root];
        let mut i = 0;
        while i < doomed.len() {
            let p = doomed[i];
            let kids: Vec<Entity> = self
                .core
                .world
                .query::<(Entity, &Parent)>()
                .iter()
                .filter(|(_, par)| par.0 == p)
                .map(|(e, _)| e)
                .collect();
            doomed.extend(kids);
            i += 1;
        }
        let input = ActionState::default();
        for &e in doomed.iter().rev() {
            // on_destroy for each behaviour of e.
            for idx in 0..self.slots.len() {
                if self.slots[idx].entity != e {
                    continue;
                }
                if let Some(mut b) = self.slots[idx].behavior.take() {
                    let mut ctx = Ctx {
                        core: &mut self.core,
                        input: &input,
                    };
                    b.on_destroy(&mut ctx, e);
                }
            }
            self.slots.retain(|s| s.entity != e);
            if let Ok(b) = self.core.world.get::<&Body>(e).map(|b| b.0) {
                self.core.physics.remove(b);
            }
            if let Ok(c) = self.core.world.get::<&Character>(e).map(|c| c.0) {
                self.core.physics.remove(c.handle);
            }
            self.core.audio.remove_emitter(e);
            if let Ok(p) = self.core.world.get::<&Persistent>(e).map(|p| p.0) {
                self.core.ids.remove(&p);
            }
            if self.core.camera_override == Some(e) {
                self.core.camera_override = None;
            }
            let _ = self.core.world.despawn(e);
        }
        self.core.order_dirty = true;
    }

    /// Recomputes world matrices parent-first. With `reset`, previous =
    /// current (no interpolation across loads or teleports).
    fn update_transforms(&mut self, reset: bool) {
        if self.core.order_dirty {
            let mut depth: HashMap<Entity, usize> = HashMap::new();
            let parents: HashMap<Entity, Entity> = self
                .core
                .world
                .query::<(Entity, &Parent)>()
                .iter()
                .map(|(e, p)| (e, p.0))
                .collect();
            let all: Vec<Entity> = self
                .core
                .world
                .query::<(Entity, &Local)>()
                .iter()
                .map(|(e, _)| e)
                .collect();
            for &e in &all {
                let mut d = 0;
                let mut c = e;
                while let Some(&p) = parents.get(&c) {
                    d += 1;
                    c = p;
                    if d > copperframe_scene::scene::MAX_DEPTH {
                        break;
                    }
                }
                depth.insert(e, d);
            }
            let mut order = all;
            order.sort_by_key(|e| depth[e]);
            self.core.order = order;
            self.core.order_dirty = false;
        }
        let mut computed: HashMap<Entity, Mat4> = HashMap::with_capacity(self.core.order.len());
        for &e in &self.core.order {
            let Ok(local) = self.core.world.get::<&Local>(e).map(|l| l.0) else {
                continue;
            };
            let parent = self
                .core
                .world
                .get::<&Parent>(e)
                .ok()
                .and_then(|p| computed.get(&p.0).copied())
                .unwrap_or(Mat4::IDENTITY);
            let m = parent * local.matrix();
            computed.insert(e, m);
            if let Ok(mut w) = self.core.world.get::<&mut WorldMatrix>(e) {
                w.previous = if reset { m } else { w.current };
                w.current = m;
            }
        }
    }

    /// Runs one fixed tick with `input`.
    pub fn tick(&mut self, input: &ActionState) {
        if self.outcome.is_some() {
            return;
        }
        self.previous_camera = self.current_camera();
        self.core.hud.prompt = None;
        if let Some((_, t)) = &mut self.core.hud.message {
            *t -= TICK_DT;
            if *t <= 0.0 {
                self.core.hud.message = None;
            }
        }
        self.core.aim = None;

        // 1. start
        for i in 0..self.slots.len() {
            if self.slots[i].started {
                continue;
            }
            self.slots[i].started = true;
            self.call(i, input, |b, ctx, me| b.start(ctx, me));
        }
        // 2. game + behaviour fixed updates
        {
            let mut ctx = Ctx {
                core: &mut self.core,
                input,
            };
            self.game.fixed_update(&mut ctx);
        }
        for i in 0..self.slots.len() {
            self.call(i, input, |b, ctx, me| b.fixed_update(ctx, me));
        }
        // 3. interaction
        self.resolve_interaction(input);
        // 4. deferred commands
        self.apply_commands(input);
        // 5. transforms and kinematic targets
        self.update_transforms(false);
        let kinematic: Vec<(BodyHandle, Mat4)> = self
            .core
            .world
            .query::<(&Body, &WorldMatrix)>()
            .iter()
            .filter(|(b, _)| b.0.kind == BodyKind::Kinematic)
            .map(|(b, w)| (b.0, w.current))
            .collect();
        for (b, m) in kinematic {
            let (_, r, t) = m.to_scale_rotation_translation();
            self.core.physics.set_kinematic_target(b, t, r);
        }
        // 6. physics
        let events = self.core.physics.step();
        let dynamic: Vec<(Entity, BodyHandle)> = self
            .core
            .world
            .query::<(Entity, &Body)>()
            .iter()
            .filter(|(_, b)| b.0.kind == BodyKind::Dynamic)
            .map(|(e, b)| (e, b.0))
            .collect();
        for (e, b) in dynamic {
            if let Some((t, r)) = self.core.physics.body_pose(b)
                && let Ok(mut l) = self.core.world.get::<&mut Local>(e)
            {
                l.0.translation = t;
                l.0.rotation = r;
            }
        }
        // 7. triggers
        for t in events.triggers {
            let sensor = entity_name(&self.core.world, t.sensor);
            let other = entity_name(&self.core.world, t.other);
            self.ctx_log(if t.entered {
                GameEvent::TriggerEntered { sensor, other }
            } else {
                GameEvent::TriggerExited { sensor, other }
            });
            for i in 0..self.slots.len() {
                if self.slots[i].entity == t.sensor {
                    self.call(i, input, |b, ctx, me| {
                        b.on_trigger(ctx, me, t.other, t.entered)
                    });
                }
            }
        }
        for c in events.contacts {
            for i in 0..self.slots.len() {
                let me = self.slots[i].entity;
                let other = if me == c.a {
                    c.b
                } else if me == c.b {
                    c.a
                } else {
                    continue;
                };
                self.call(i, input, |b, ctx, me| {
                    b.on_collision(ctx, me, other, c.started)
                });
            }
        }
        self.apply_commands(input);
        // 8. animation, transforms, audio
        let mut finished = Vec::new();
        for (e, animator) in self.core.world.query_mut::<(Entity, &mut Animator)>() {
            if let Some(f) = animator.advance(TICK_DT) {
                finished.push((e, f.clip));
            }
        }
        for (e, clip) in finished {
            let entity = entity_name(&self.core.world, e);
            self.ctx_log(GameEvent::AnimationFinished { entity, clip });
        }
        self.update_transforms(false);
        let emitters: Vec<(Entity, Vec3)> = self
            .core
            .world
            .query::<(Entity, &Emitter, &WorldMatrix)>()
            .iter()
            .map(|(e, _, w)| (e, w.current.w_axis.truncate()))
            .collect();
        for (e, p) in emitters {
            self.core.audio.set_emitter_position(e, p);
        }
        let cam = self.current_camera();
        self.core.audio.set_listener(cam.position, cam.rotation());
        self.core.audio.update(f64::from(TICK_DT));
        self.core.tick += 1;

        if let Some(path) = self.pending_scene.take()
            && let Err(e) = self.load_scene_file(Path::new(&path))
        {
            self.warnings.push(e.to_string());
            tracing::error!("{e}");
        }
    }

    /// Calls a behaviour with a context. The behaviour is taken out of its
    /// slot for the call so it can freely use the context.
    fn call(
        &mut self,
        i: usize,
        input: &ActionState,
        f: impl FnOnce(&mut dyn Behavior, &mut Ctx, Entity),
    ) {
        let Some(slot) = self.slots.get_mut(i) else {
            return;
        };
        let me = slot.entity;
        let Some(mut b) = slot.behavior.take() else {
            return;
        };
        if self.core.world.contains(me) {
            let mut ctx = Ctx {
                core: &mut self.core,
                input,
            };
            f(b.as_mut(), &mut ctx, me);
        }
        // The slot list does not change during a call (removals are
        // deferred), so index i is still this behaviour's slot.
        if let Some(slot) = self.slots.get_mut(i)
            && slot.entity == me
        {
            slot.behavior = Some(b);
        }
    }

    fn resolve_interaction(&mut self, input: &ActionState) {
        let Some(aim) = self.core.aim else { return };
        let hit = {
            let ctx = Ctx {
                core: &mut self.core,
                input,
            };
            ctx.raycast(aim.origin, aim.direction, aim.max_distance, Some(aim.by))
        };
        let Some(hit) = hit else { return };
        // The hit entity or its nearest ancestor with a prompt.
        let mut current = Some(hit.entity);
        let mut found = None;
        while let Some(e) = current {
            if let Some(prompt) = self
                .slots
                .iter()
                .filter(|s| s.entity == e)
                .find_map(|s| s.behavior.as_ref().and_then(|b| b.interaction_prompt()))
            {
                found = Some((e, prompt));
                break;
            }
            current = self.core.world.get::<&Parent>(e).ok().map(|p| p.0);
        }
        let Some((target, prompt)) = found else {
            return;
        };
        let key = crate::actions::Bindings::default()
            .buttons(names::INTERACT)
            .first()
            .map(|b| b.label())
            .unwrap_or_default();
        self.core.hud.prompt = Some(if key.is_empty() {
            prompt
        } else {
            format!("[{key}] {prompt}")
        });
        if input.pressed(names::INTERACT) {
            self.core
                .commands
                .push(Cmd::Interact { target, by: aim.by });
        }
    }

    /// Applies deferred commands; commands created while applying are
    /// processed too, up to a bound (to stop message ping-pong loops).
    fn apply_commands(&mut self, input: &ActionState) {
        for _round in 0..16 {
            let commands = std::mem::take(&mut self.core.commands);
            if commands.is_empty() {
                return;
            }
            let mut spawned = Vec::new();
            for cmd in commands {
                match cmd {
                    Cmd::Spawn(e) => {
                        if let Some(h) = self.spawn_now(*e) {
                            spawned.push(h);
                        }
                    }
                    Cmd::Despawn(e) => self.despawn_now(e),
                    Cmd::Message { to, from, text } => {
                        for i in 0..self.slots.len() {
                            if self.slots[i].entity == to {
                                self.call(i, input, |b, ctx, me| {
                                    b.on_message(ctx, me, &text, from)
                                });
                            }
                        }
                    }
                    Cmd::Interact { target, by } => {
                        let name = entity_name(&self.core.world, target);
                        self.ctx_log(GameEvent::Interacted { entity: name });
                        for i in 0..self.slots.len() {
                            if self.slots[i].entity == target {
                                self.call(i, input, |b, ctx, me| b.on_interact(ctx, me, by));
                            }
                        }
                    }
                    Cmd::LoadScene(path) => self.pending_scene = Some(path),
                    Cmd::Finish(message) => {
                        self.ctx_log(GameEvent::Finished {
                            message: message.clone(),
                        });
                        self.outcome = Some(Outcome::Finished { message });
                    }
                    Cmd::Quit => self.outcome = Some(Outcome::Quit),
                }
            }
            if !spawned.is_empty() {
                self.update_transforms(false);
                // New entities have no previous pose yet.
                for &h in &spawned {
                    if let Ok(mut w) = self.core.world.get::<&mut WorldMatrix>(h) {
                        w.previous = w.current;
                    }
                }
                self.create_bodies_and_emitters(Some(&spawned));
                self.core.physics.refresh();
            }
        }
        tracing::warn!("command processing stopped after 16 rounds (message loop?)");
        self.core.commands.clear();
    }

    /// Ends the session: `Game::shutdown` and every behaviour's
    /// `on_destroy`.
    pub fn shutdown(&mut self) {
        let input = ActionState::default();
        {
            let mut ctx = Ctx {
                core: &mut self.core,
                input: &input,
            };
            self.game.shutdown(&mut ctx);
        }
        self.unload();
    }

    /// Restarts the current scene from its file (the title screen's
    /// "New game"). Game state is kept unless `reset_state`.
    pub fn restart(&mut self, reset_state: bool) -> Result<(), LoadError> {
        self.outcome = None;
        if reset_state {
            self.core.state.clear();
        }
        let Some(path) = self.scene_path.clone() else {
            return Ok(());
        };
        let scene = self.core.scene.as_deref().cloned();
        match scene {
            Some(scene) => self.load_scene(scene, &path),
            None => self.load_scene_file(&path),
        }
    }
}

impl Assets {
    fn model(&mut self, asset: AssetId) -> Option<Arc<ImportedModel>> {
        if let Some(m) = self.models.get(&asset) {
            return m.clone();
        }
        let loaded = self.db.get(asset).and_then(|entry| {
            match AssetDatabase::import(&self.project, entry) {
                Ok(ImportedAsset::Model(m)) => Some(Arc::new(m)),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            }
        });
        self.models.insert(asset, loaded.clone());
        loaded
    }

    fn triangles(&mut self, source: &MeshSource) -> Option<MeshTriangles> {
        match source {
            MeshSource::Primitive { shape, .. } => {
                let m = copperframe_runtime::scene_gpu::primitive_mesh(*shape);
                Some(MeshTriangles {
                    positions: m.vertices.iter().map(|v| Vec3::from(v.position)).collect(),
                    indices: m
                        .indices
                        .chunks_exact(3)
                        .map(|t| [t[0], t[1], t[2]])
                        .collect(),
                })
            }
            MeshSource::Model { asset } => {
                let m = self.model(*asset)?;
                let mut out = MeshTriangles {
                    positions: Vec::new(),
                    indices: Vec::new(),
                };
                for part in &m.parts {
                    let base = out.positions.len() as u32;
                    // Skinned parts collide in their rest pose.
                    out.positions.extend(m.rest_positions(part));
                    out.indices.extend(
                        part.mesh
                            .indices
                            .chunks_exact(3)
                            .map(|t| [base + t[0], base + t[1], base + t[2]]),
                    );
                }
                Some(out)
            }
        }
    }

    fn load_sound(&mut self, audio: &mut Audio<Entity>, asset: AssetId) -> Result<(), String> {
        if self.sounds.contains(&asset) {
            return Ok(());
        }
        let entry = self
            .db
            .get(asset)
            .ok_or_else(|| format!("asset {asset} is not in the project"))?;
        if entry.kind != AssetKind::Audio {
            return Err(format!("{} is not a sound", entry.relative));
        }
        match AssetDatabase::import(&self.project, entry) {
            Ok(ImportedAsset::Audio(a)) => {
                audio.load(asset, a.bytes)?;
                self.sounds.insert(asset);
                Ok(())
            }
            Ok(_) => Err(format!("{} is not a sound", entry.relative)),
            Err(e) => Err(e.to_string()),
        }
    }
}
