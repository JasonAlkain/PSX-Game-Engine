//! Headless simulation: scene loading, player movement and interaction,
//! kinematic doors, triggers, sounds, spawning with ID remapping, scene
//! transitions with cleanup, and play/stop isolation of the authored file.

use copperframe_assets::import::audio::wav_bytes;
use copperframe_assets::project::{AssetDatabase, Project};
use copperframe_core::EntityId;
use copperframe_game::actions::{ActionState, names};
use copperframe_game::audio::Audio;
use copperframe_game::{
    Behavior, BehaviorDescriptor, Ctx, Entity, FieldDescriptor, Game, GameEvent, Outcome, Registry,
    Simulation,
};
use copperframe_scene::{
    AudioSourceComponent, BehaviorComponent, BodyKind, ColliderComponent, ColliderShape,
    Entity as SceneEntity, MeshComponent, MeshSource, PrimitiveShape, Scene, Transform,
};
use glam::{Vec2, Vec3};

struct Door {
    open: bool,
    closed_y: Option<f32>,
}
impl Behavior for Door {
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        let Some(mut t) = ctx.local(me) else { return };
        let closed = *self.closed_y.get_or_insert(t.translation.y);
        let target = if self.open { closed + 3.0 } else { closed };
        let step = 2.0 * ctx.dt();
        t.translation.y += (target - t.translation.y).clamp(-step, step);
        ctx.set_local(me, t);
    }
    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, _from: Option<Entity>) {
        if message == "toggle" {
            self.open = !self.open;
            ctx.play_sound(me);
        }
    }
}

struct Switch {
    target: Option<EntityId>,
}
impl Behavior for Switch {
    fn interaction_prompt(&self) -> Option<String> {
        Some("Pull lever".into())
    }
    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, _by: Entity) {
        if let Some(door) = self.target.and_then(|id| ctx.find(id)) {
            ctx.send(door, Some(me), "toggle");
        }
    }
}

struct Exit;
impl Behavior for Exit {
    fn on_trigger(&mut self, ctx: &mut Ctx, _me: Entity, other: Entity, entered: bool) {
        if entered
            && ctx
                .world()
                .get::<&copperframe_game::builtin::PlayerTag>(other)
                .is_ok()
        {
            ctx.finish("You escaped");
        }
    }
}

struct TestGame;
impl Game for TestGame {
    fn title(&self) -> String {
        "Test".into()
    }
    fn register(&self, r: &mut Registry) {
        r.register(BehaviorDescriptor {
            kind: "test.door",
            description: "door",
            fields: vec![],
            create: |_| {
                Ok(Box::new(Door {
                    open: false,
                    closed_y: None,
                }))
            },
        });
        r.register(BehaviorDescriptor {
            kind: "test.switch",
            description: "switch",
            fields: vec![FieldDescriptor::entity("target", "door")],
            create: |p| {
                Ok(Box::new(Switch {
                    target: p.entity("target"),
                }))
            },
        });
        r.register(BehaviorDescriptor {
            kind: "test.exit",
            description: "exit",
            fields: vec![],
            create: |_| Ok(Box::new(Exit)),
        });
    }
}

fn boxed(name: &str, pos: Vec3, size: Vec3, body: BodyKind) -> SceneEntity {
    let mut e = SceneEntity::new(name);
    e.transform = Transform::from_translation(pos);
    e.transform.scale = size;
    e.mesh = Some(MeshComponent {
        source: MeshSource::Primitive {
            shape: PrimitiveShape::Cube,
            texture: None,
        },
        material: Default::default(),
    });
    e.collider = Some(ColliderComponent::new(
        ColliderShape::Box { size: Vec3::ONE },
        body,
    ));
    e
}

/// A corridor along -Z: player at z=4, lever at x=1.2 z=1 (facing it means
/// turning right), door across the corridor at z=-1, exit zone at z=-4.
fn build(root: &std::path::Path) -> (Project, Scene, [EntityId; 4]) {
    let _ = std::fs::remove_dir_all(root);
    let project = Project::create(root, "Sim").unwrap();
    let sound = root.join("assets/door.wav");
    let samples: Vec<f32> = (0..2205).map(|i| (i as f32 * 0.2).sin() * 0.3).collect();
    std::fs::write(&sound, wav_bytes(22050, &samples)).unwrap();
    let sound_id = AssetDatabase::register(&project, &sound).unwrap().id;

    let mut scene = Scene::new("Corridor");
    let floor = boxed(
        "Floor",
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(4.0, 1.0, 12.0),
        BodyKind::Static,
    );
    let wall_l = boxed(
        "Wall L",
        Vec3::new(-1.5, 1.5, 0.0),
        Vec3::new(0.2, 3.0, 12.0),
        BodyKind::Static,
    );
    let wall_r = boxed(
        "Wall R",
        Vec3::new(1.5, 1.5, 0.0),
        Vec3::new(0.2, 3.0, 12.0),
        BodyKind::Static,
    );
    let mut door = boxed(
        "Door",
        Vec3::new(0.0, 1.25, -1.0),
        Vec3::new(2.8, 2.5, 0.2),
        BodyKind::Kinematic,
    );
    door.behaviors = vec![BehaviorComponent::new("test.door")];
    let mut audio = AudioSourceComponent::new(sound_id);
    audio.spatial = true;
    door.audio = Some(audio);
    let mut lever = boxed(
        "Lever",
        Vec3::new(1.3, 1.2, 1.0),
        Vec3::new(0.2, 0.4, 0.6),
        BodyKind::Static,
    );
    lever.behaviors =
        vec![BehaviorComponent::new("test.switch").with("target", door.id.to_string())];
    let mut exit = SceneEntity::new("Exit");
    exit.transform = Transform::from_translation(Vec3::new(0.0, 1.0, -4.0));
    let mut zone = ColliderComponent::new(
        ColliderShape::Box {
            size: Vec3::new(2.5, 2.0, 1.0),
        },
        BodyKind::Static,
    );
    zone.sensor = true;
    exit.collider = Some(zone);
    exit.behaviors = vec![BehaviorComponent::new("test.exit")];
    let mut player = SceneEntity::new("Player");
    player.transform = Transform::from_translation(Vec3::new(0.0, 0.0, 4.0));
    player.behaviors = vec![BehaviorComponent::new("psx.player")];
    let ids = [player.id, door.id, lever.id, exit.id];
    scene.entities = vec![floor, wall_l, wall_r, door, lever, exit, player];
    assert!(scene.problems().is_empty(), "{:?}", scene.problems());
    let path = project.startup_scene_path();
    copperframe_scene::save_scene(&scene, &path).unwrap();
    (project, scene, ids)
}

fn run(sim: &mut Simulation, ticks: u32, held: &[&str], look: Vec2) {
    for _ in 0..ticks {
        sim.tick(&ActionState::scripted(held, &[], look));
    }
}

#[test]
fn walk_pull_lever_open_door_and_exit() {
    let root = std::env::temp_dir().join(format!("psx_sim_{}", std::process::id()));
    let (project, _scene, [player, door, _lever, _exit]) = build(&root);
    let scene_path = project.startup_scene_path();
    let authored = std::fs::read(&scene_path).unwrap();

    let mut sim = Simulation::new(project.clone(), Box::new(TestGame), Audio::silent());
    sim.load_scene_file(&scene_path).unwrap();
    assert!(sim.take_warnings().is_empty());
    assert_eq!(
        sim.body_count(),
        6,
        "the player capsule is created on its first tick"
    );
    run(&mut sim, 1, &[], Vec2::ZERO);
    assert_eq!(sim.body_count(), 7, "player capsule created on start");

    // Walk to z = 1.
    run(&mut sim, 72, &[names::MOVE_FORWARD], Vec2::ZERO);
    let p = sim.find(player).unwrap();
    let pos = sim
        .world()
        .get::<&copperframe_game::sim::Local>(p)
        .unwrap()
        .0
        .translation;
    assert!((pos.z - 1.0).abs() < 0.2 && pos.y.abs() < 0.05, "{pos:?}");
    // The closed door blocks the way.
    run(&mut sim, 120, &[names::MOVE_FORWARD], Vec2::ZERO);
    let pos = sim
        .world()
        .get::<&copperframe_game::sim::Local>(p)
        .unwrap()
        .0
        .translation;
    assert!(pos.z > -0.95, "door blocks: {pos:?}");
    // Back up, turn right 90° (look x = +90° over 9 ticks) to face the lever.
    run(&mut sim, 36, &[names::MOVE_BACK], Vec2::ZERO);
    run(&mut sim, 9, &[], Vec2::new(10.0, -2.0));
    run(&mut sim, 1, &[], Vec2::ZERO);
    assert_eq!(sim.hud().prompt.as_deref(), Some("[E] Pull lever"));
    sim.tick(&ActionState::scripted(&[], &[names::INTERACT], Vec2::ZERO));
    let events = sim.take_events();
    assert!(events.iter().any(|e| e.event
        == GameEvent::Interacted {
            entity: "Lever".into()
        }));
    assert!(events.iter().any(|e| e.event
        == GameEvent::SoundStarted {
            entity: "Door".into()
        }));
    // Door rises; turn back and walk to the exit.
    run(&mut sim, 120, &[], Vec2::ZERO);
    let d = sim.find(door).unwrap();
    let dy = sim
        .world()
        .get::<&copperframe_game::sim::Local>(d)
        .unwrap()
        .0
        .translation
        .y;
    assert!(dy > 4.0, "door opened: {dy}");
    run(&mut sim, 9, &[], Vec2::new(-10.0, 2.0));
    run(
        &mut sim,
        200,
        &[names::MOVE_FORWARD, names::RUN],
        Vec2::ZERO,
    );
    assert_eq!(
        sim.outcome(),
        Some(&Outcome::Finished {
            message: "You escaped".into()
        })
    );
    let events = sim.take_events();
    assert!(
        events.iter().any(
            |e| matches!(&e.event, GameEvent::TriggerEntered { sensor, .. } if sensor == "Exit")
        )
    );

    // Stopping never touched the authored scene.
    sim.shutdown();
    assert_eq!(sim.entity_count(), 0);
    assert_eq!(sim.body_count(), 0);
    assert_eq!(sim.emitter_count(), 0);
    assert_eq!(std::fs::read(&scene_path).unwrap(), authored);
    std::fs::remove_dir_all(&root).unwrap();
}

struct Spawner {
    template: Option<EntityId>,
    spawned: Option<EntityId>,
}
impl Behavior for Spawner {
    fn fixed_update(&mut self, ctx: &mut Ctx, _me: Entity) {
        if ctx.tick() == 2 {
            let t = Transform::from_translation(Vec3::new(5.0, 0.0, 0.0));
            self.spawned = self.template.and_then(|id| ctx.instantiate(id, t));
            ctx.state_mut().insert(
                "spawned".into(),
                serde_json::json!(self.spawned.map(|i| i.to_string())),
            );
        }
        if ctx.tick() == 4 {
            ctx.load_scene("scenes/second.scene.json");
        }
    }
}

struct SpawnGame;
impl Game for SpawnGame {
    fn title(&self) -> String {
        "Spawn".into()
    }
    fn register(&self, r: &mut Registry) {
        TestGame.register(r);
        r.register(BehaviorDescriptor {
            kind: "test.spawner",
            description: "spawner",
            fields: vec![FieldDescriptor::entity("template", "what")],
            create: |p| {
                Ok(Box::new(Spawner {
                    template: p.entity("template"),
                    spawned: None,
                }))
            },
        });
    }
}

#[test]
fn instantiate_remaps_references_and_scene_change_cleans_up() {
    let root = std::env::temp_dir().join(format!("psx_sim_spawn_{}", std::process::id()));
    let (project, mut scene, [_, door, lever, _]) = build(&root);
    // Template: the door with the lever re-parented under it.
    scene.get_mut(lever).unwrap().parent = Some(door);
    scene.get_mut(lever).unwrap().collider = None;
    let mut spawner = SceneEntity::new("Spawner");
    spawner.behaviors =
        vec![BehaviorComponent::new("test.spawner").with("template", door.to_string())];
    scene.entities.push(spawner);
    // The kinematic door is a root; fine for the template.
    assert!(scene.problems().is_empty(), "{:?}", scene.problems());
    let second = Scene::new("Second");
    copperframe_scene::save_scene(&second, &root.join("scenes/second.scene.json")).unwrap();

    let mut sim = Simulation::new(project, Box::new(SpawnGame), Audio::silent());
    let path = root.join("scenes/main.scene.json");
    sim.load_scene(scene.clone(), &path).unwrap();
    let before = (sim.entity_count(), sim.body_count(), sim.emitter_count());
    assert_eq!(sim.cached_assets().1, 1, "the door sound is decoded");
    run(&mut sim, 4, &[], Vec2::ZERO);
    // A copy of door + lever exists with fresh IDs.
    assert_eq!(sim.entity_count(), before.0 + 2);
    assert_eq!(
        sim.body_count(),
        before.1 + 1 + 1,
        "door copy body + player capsule"
    );
    assert_eq!(sim.emitter_count(), before.2 + 1);
    let new_door: EntityId = sim.state()["spawned"].as_str().unwrap().parse().unwrap();
    assert_ne!(new_door, door);
    let copy = sim.find(new_door).unwrap();
    let copy_pos = sim
        .world()
        .get::<&copperframe_game::sim::WorldMatrix>(copy)
        .unwrap()
        .current
        .w_axis;
    assert_eq!(copy_pos.x, 5.0);
    // The copied lever targets the copied door, not the original.
    let copied_lever = sim
        .world()
        .query::<(
            &copperframe_game::sim::Parent,
            &copperframe_game::sim::Source,
        )>()
        .iter()
        .find(|(p, _)| p.0 == copy)
        .map(|(_, s)| s.0.clone())
        .unwrap();
    assert_eq!(
        copied_lever.behaviors[0].params["target"],
        serde_json::json!(new_door.to_string())
    );

    // Tick 4 requests the second scene: everything from the first is gone,
    // game state survives.
    run(&mut sim, 1, &[], Vec2::ZERO);
    assert_eq!(sim.scene().unwrap().name, "Second");
    assert_eq!(sim.entity_count(), 0);
    assert_eq!(sim.body_count(), 0);
    assert_eq!(sim.emitter_count(), 0);
    assert!(sim.state().contains_key("spawned"));
    // The second scene uses no assets: cached models and sounds are freed.
    assert_eq!(sim.cached_assets(), (0, 0));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unknown_behaviours_and_bad_parameters_refuse_to_load() {
    let root = std::env::temp_dir().join(format!("psx_sim_bad_{}", std::process::id()));
    let (project, mut scene, [player, ..]) = build(&root);
    scene
        .get_mut(player)
        .unwrap()
        .behaviors
        .push(BehaviorComponent::new("nope.missing"));
    scene.entities[0].behaviors = vec![BehaviorComponent::new("psx.player").with("speed", "x")];
    let mut sim = Simulation::new(project, Box::new(TestGame), Audio::silent());
    let err = sim
        .load_scene(scene, &root.join("scenes/main.scene.json"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("nope.missing") && err.contains("speed"),
        "{err}"
    );
    assert_eq!(sim.entity_count(), 0);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Plays `bend` once on its own entity at tick 3, with a short fade.
struct Waver;
impl Behavior for Waver {
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        if ctx.tick() == 3 {
            assert!(!ctx.play_animation(me, "nope", 0.1, false));
            let ok = ctx.play_animation(me, "bend", 0.1, false);
            ctx.state_mut()
                .insert("played".into(), serde_json::json!(ok));
        }
    }
}

struct AnimGame;
impl Game for AnimGame {
    fn title(&self) -> String {
        "Anim".into()
    }
    fn register(&self, r: &mut Registry) {
        r.register(BehaviorDescriptor {
            kind: "test.waver",
            description: "waves",
            fields: vec![],
            create: |_| Ok(Box::new(Waver)),
        });
    }
}

#[test]
fn animated_models_play_crossfade_and_report_finished_clips() {
    use copperframe_assets::fixtures::Fixture;
    use copperframe_scene::{AnimatorComponent, LightComponent, ShadowComponent};

    let root = std::env::temp_dir().join(format!("psx_sim_anim_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = Project::create(&root, "Anim").unwrap();
    let file = root.join("assets/arm.glb");
    std::fs::write(&file, Fixture::skinned_arm().to_glb()).unwrap();
    let arm = AssetDatabase::register(&project, &file).unwrap().id;

    let mut scene = Scene::new("Arm");
    let mut actor = SceneEntity::new("Arm");
    actor.mesh = Some(MeshComponent {
        source: MeshSource::Model { asset: arm },
        material: Default::default(),
    });
    let mut idle = AnimatorComponent::new("lift");
    idle.autoplay = false;
    actor.animator = Some(idle);
    actor.shadow = Some(ShadowComponent::default());
    actor.behaviors = vec![BehaviorComponent::new("test.waver")];
    // A clip the model does not have is a load warning, not an error.
    let mut other = actor.clone();
    other.id = EntityId::new();
    other.name = "Other".into();
    other.behaviors.clear();
    other.animator = Some(AnimatorComponent::new("missing"));
    let mut lamp = SceneEntity::new("Lamp");
    lamp.transform = Transform::from_translation(Vec3::new(0.0, 2.0, 1.0));
    lamp.light = Some(LightComponent::default());
    scene.entities = vec![actor, other, lamp];
    assert!(scene.problems().is_empty(), "{:?}", scene.problems());

    let mut sim = Simulation::new(project, Box::new(AnimGame), Audio::silent());
    sim.load_scene(scene, &root.join("scenes/main.scene.json"))
        .unwrap();
    let warnings = sim.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("missing") && w.contains("bend, lift")),
        "{warnings:?}"
    );
    assert_eq!(sim.lights(1.0).len(), 1);
    assert_eq!(sim.shadows(1.0).len(), 2);

    let palette = |sim: &Simulation, name: &str| {
        sim.draw_list()
            .into_iter()
            .find(|d| {
                sim.find(d.id).is_some_and(|e| {
                    sim.world()
                        .get::<&copperframe_game::sim::Name>(e)
                        .unwrap()
                        .0
                        == name
                })
            })
            .and_then(|d| d.joints)
            .unwrap()
    };
    let rest = palette(&sim, "Arm");
    run(&mut sim, 3, &[], Vec2::ZERO);
    // Held on `lift`'s first frame (autoplay off): still the rest pose.
    assert_eq!(palette(&sim, "Arm"), rest);
    run(&mut sim, 80, &[], Vec2::ZERO);
    assert_eq!(sim.state()["played"], serde_json::json!(true));
    assert_ne!(palette(&sim, "Arm"), rest);
    let finished: Vec<_> = sim
        .take_events()
        .into_iter()
        .filter_map(|e| match e.event {
            GameEvent::AnimationFinished { entity, clip } => Some((entity, clip)),
            _ => None,
        })
        .collect();
    assert_eq!(finished, vec![("Arm".to_owned(), "bend".to_owned())]);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn repeated_restarts_do_not_accumulate_entities_bodies_or_emitters() {
    let root = std::env::temp_dir().join(format!("psx_sim_restart_{}", std::process::id()));
    let (project, scene, _) = build(&root);
    let mut sim = Simulation::new(project, Box::new(TestGame), Audio::silent());
    sim.load_scene(scene, &root.join("scenes/main.scene.json"))
        .unwrap();
    run(&mut sim, 30, &["move_forward"], Vec2::ZERO);
    let first = (
        sim.entity_count(),
        sim.body_count(),
        sim.emitter_count(),
        sim.cached_assets(),
    );
    for _ in 0..50 {
        sim.restart(false).unwrap();
        run(&mut sim, 30, &["move_forward"], Vec2::ZERO);
        let now = (
            sim.entity_count(),
            sim.body_count(),
            sim.emitter_count(),
            sim.cached_assets(),
        );
        assert_eq!(now, first);
        assert!(sim.take_warnings().is_empty());
        sim.take_events();
    }
    std::fs::remove_dir_all(&root).unwrap();
}
