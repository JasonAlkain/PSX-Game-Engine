//! Lua behaviours: registration and schema, fields, the ctx API, messages,
//! game state, diagnostics for load and runtime errors, the instruction
//! budget, the sandbox, determinism, and the project switch.

use std::path::{Path, PathBuf};

use copperframe_assets::project::Project;
use copperframe_game::actions::ActionState;
use copperframe_game::audio::Audio;
use copperframe_game::headless::{self, Script};
use copperframe_game::{Game, GameEvent, Registry, Simulation};
use copperframe_scene::{BehaviorComponent, Entity as SceneEntity, Scene, Transform};
use glam::{EulerRot, Vec3};

struct NoGame;
impl Game for NoGame {
    fn title(&self) -> String {
        "Lua test".into()
    }
    fn register(&self, _: &mut Registry) {}
}

/// A fresh project with `scripts` written under `scripts/`.
fn project(name: &str, scripts: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cf_lua_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Project::create(&root, name).unwrap();
    for (file, text) in scripts {
        let path = root.join("scripts").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn entity(name: &str, behaviors: Vec<BehaviorComponent>) -> SceneEntity {
    let mut e = SceneEntity::new(name);
    e.behaviors = behaviors;
    e
}

fn sim_with(root: &Path, entities: Vec<SceneEntity>) -> Simulation {
    let mut scene = Scene::new("Lua");
    scene.entities = entities;
    let path = root.join("scenes/lua.scene.json");
    copperframe_scene::save_scene(&scene, &path).unwrap();
    let mut sim = Simulation::new(
        Project::open(root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    sim.load_scene_file(&path).unwrap();
    sim
}

fn run(sim: &mut Simulation, ticks: u32) {
    for _ in 0..ticks {
        sim.tick(&ActionState::default());
    }
}

const SPIN: &str = r#"
return {
  description = "Turns about Y",
  fields = {
    { name = "speed", default = 45, help = "degrees per second" },
    { name = "enabled", default = true },
    { name = "label", default = "spinner" },
    { name = "partner", kind = "entity", help = "another entity" },
  },
  fixed_update = function(self, ctx)
    if self.enabled then ctx:rotate(self.entity, self.speed * ctx:dt()) end
  end,
}
"#;

#[test]
fn scripts_register_with_fields_and_folders_in_the_schema() {
    let root = project(
        "schema",
        &[("spin.lua", SPIN), ("puzzles/door.lua", "return {}")],
    );
    let sim = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    let schema = sim.registry().schema("t");
    let kinds = schema["behaviors"].as_array().unwrap();
    let spin = kinds.iter().find(|b| b["kind"] == "lua.spin").unwrap();
    assert_eq!(spin["description"], "Turns about Y");
    assert_eq!(spin["folder"], "Scripts");
    let fields = spin["fields"].as_array().unwrap();
    let kinds_of: Vec<(&str, &str)> = fields
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        kinds_of,
        [
            ("speed", "number"),
            ("enabled", "bool"),
            ("label", "text"),
            ("partner", "entity")
        ]
    );
    assert_eq!(fields[0]["default"], 45.0);
    assert_eq!(fields[0]["help"], "degrees per second");
    let door = kinds
        .iter()
        .find(|b| b["kind"] == "lua.puzzles.door")
        .unwrap();
    assert_eq!(door["folder"], "Scripts/puzzles");
    assert!(schema.get("problems").is_none());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_script_moves_its_entity_with_scene_parameters() {
    let root = project("spin", &[("spin.lua", SPIN)]);
    let spinner = entity(
        "Spinner",
        vec![BehaviorComponent::new("lua.spin").with("speed", 90.0)],
    );
    let id = spinner.id;
    let mut sim = sim_with(&root, vec![spinner]);
    run(&mut sim, 61); // start, then 60 updates = 1 s
    let e = sim.find(id).unwrap();
    let t = *sim.world().get::<&copperframe_game::sim::Local>(e).unwrap();
    let (yaw, _, _) = t.0.rotation.to_euler(EulerRot::YXZ);
    assert!(
        (yaw.to_degrees() - 90.0).abs() < 2.0,
        "yaw {}",
        yaw.to_degrees()
    );
    assert!(sim.take_diagnostics().is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

const LEVER: &str = r#"
return {
  fields = { { name = "door", kind = "entity" } },
  start = function(self, ctx)
    self.prompt = "Pull"
    local door = ctx:find(self.door)
    ctx:send(door, "open")
    ctx:set_state("lever_pulled", true)
  end,
}
"#;
const DOOR: &str = r#"
local M = {}
function M:open(ctx)
  local x, y, z = ctx:position(self.entity)
  ctx:set_position(self.entity, x, y + 2, z)
  ctx:event("door_opened", ctx:name(self.entity))
  ctx:set_state("opened_at", ctx:tick())
end
function M.on_message(self, ctx, message, from)
  if message == "open" and ctx:name(from) == "Lever" then self:open(ctx) end
end
return M
"#;

#[test]
fn scripts_message_each_other_log_events_and_keep_game_state() {
    let root = project("msg", &[("lever.lua", LEVER), ("door.lua", DOOR)]);
    let mut door = entity("Door", vec![BehaviorComponent::new("lua.door")]);
    door.transform = Transform::from_translation(Vec3::new(0.0, 1.0, 0.0));
    let door_id = door.id;
    let lever = entity(
        "Lever",
        vec![BehaviorComponent::new("lua.lever").with("door", door_id.to_string())],
    );
    let mut sim = sim_with(&root, vec![lever, door]);
    run(&mut sim, 3);
    assert!(sim.take_diagnostics().is_empty());
    let e = sim.find(door_id).unwrap();
    let y = sim
        .world()
        .get::<&copperframe_game::sim::Local>(e)
        .unwrap()
        .0
        .translation
        .y;
    assert!((y - 3.0).abs() < 1e-5, "door at {y}");
    assert_eq!(sim.state()["lever_pulled"], true);
    assert!(sim.state()["opened_at"].is_number());
    assert!(sim.take_events().iter().any(|t| matches!(
        &t.event,
        GameEvent::Custom { name, detail } if name == "door_opened" && detail == "Door"
    )));
    std::fs::remove_dir_all(&root).unwrap();
}

const BROKEN_AT_TICK_3: &str = r#"
return {
  fixed_update = function(self, ctx)
    if ctx:tick() == 3 then
      local t = nil
      return t.field
    end
    ctx:set_state("still_running", ctx:tick())
  end,
}
"#;
const COUNTER: &str = r#"
return {
  fixed_update = function(self, ctx)
    ctx:set_state("counter", (ctx:get_state("counter") or 0) + 1)
  end,
}
"#;

#[test]
fn a_runtime_error_switches_off_only_that_behaviour_and_is_reported_once() {
    let root = project(
        "err",
        &[("broken.lua", BROKEN_AT_TICK_3), ("counter.lua", COUNTER)],
    );
    let mut sim = sim_with(
        &root,
        vec![
            entity("Bad", vec![BehaviorComponent::new("lua.broken")]),
            entity("Good", vec![BehaviorComponent::new("lua.counter")]),
        ],
    );
    run(&mut sim, 20);
    let d = sim.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].file, "scripts/broken.lua");
    assert_eq!(d[0].line, Some(6));
    assert_eq!(d[0].kind, "lua.broken");
    assert_eq!(d[0].entity.as_deref(), Some("Bad"));
    assert_eq!(d[0].callback.as_deref(), Some("fixed_update"));
    assert_eq!(d[0].tick, Some(3));
    assert!(d[0].message.contains("nil"), "{}", d[0].message);
    assert_eq!(sim.state()["still_running"], 2, "off after the error");
    assert_eq!(sim.state()["counter"], 20, "the other script kept running");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_bad_argument_to_ctx_reports_the_script_line() {
    let script = "return {\n  start = function(self, ctx)\n    ctx:set_position(self.entity, 1, 'up', 3)\n  end,\n}\n";
    let root = project("arg", &[("bad_arg.lua", script)]);
    let mut sim = sim_with(
        &root,
        vec![entity("E", vec![BehaviorComponent::new("lua.bad_arg")])],
    );
    run(&mut sim, 2);
    let d = sim.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, Some(3), "{d:?}");
    assert!(d[0].message.contains("set_position"), "{}", d[0].message);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_endless_loop_is_stopped_and_the_game_goes_on() {
    let script = "return { fixed_update = function(self, ctx) while true do end end }";
    let root = project("loop", &[("forever.lua", script), ("counter.lua", COUNTER)]);
    let mut sim = sim_with(
        &root,
        vec![
            entity("Loop", vec![BehaviorComponent::new("lua.forever")]),
            entity("Good", vec![BehaviorComponent::new("lua.counter")]),
        ],
    );
    run(&mut sim, 5);
    let d = sim.take_diagnostics();
    assert_eq!(d.len(), 1);
    assert!(d[0].message.contains("instructions"), "{}", d[0].message);
    assert_eq!(sim.state()["counter"], 5);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn scripts_that_do_not_load_are_reported_and_not_registered() {
    let root = project(
        "load",
        &[
            (
                "syntax.lua",
                "return {\n  start = function(self, ctx)\n    if then\n  end,\n}\n",
            ),
            ("not_table.lua", "return 42"),
            ("bad_field.lua", "return { fields = { { name = \"x\" } } }"),
            ("bad_callback.lua", "return { start = 3 }"),
            ("ok.lua", "return {}"),
        ],
    );
    let mut sim = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    let d = sim.take_diagnostics();
    let by_file = |f: &str| d.iter().find(|d| d.file == format!("scripts/{f}")).unwrap();
    assert_eq!(d.len(), 4, "{d:?}");
    assert_eq!(by_file("syntax.lua").line, Some(3));
    assert!(
        by_file("not_table.lua")
            .message
            .contains("must return a table")
    );
    assert!(by_file("bad_field.lua").message.contains("`x`"));
    assert!(
        by_file("bad_callback.lua")
            .message
            .contains("`start` must be a function")
    );
    assert!(sim.registry().get("lua.ok").is_some());
    assert!(sim.registry().get("lua.syntax").is_none());

    // A scene that uses a broken script does not load, and a headless run
    // reports why.
    let mut scene = Scene::new("Broken");
    scene.entities = vec![entity("E", vec![BehaviorComponent::new("lua.syntax")])];
    let path = root.join("scenes/broken.scene.json");
    copperframe_scene::save_scene(&scene, &path).unwrap();
    let err = sim.load_scene_file(&path).unwrap_err().to_string();
    assert!(err.contains("lua.syntax"), "{err}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn headless_reports_fail_on_script_errors() {
    let root = project("headless", &[("broken.lua", BROKEN_AT_TICK_3)]);
    let mut sim = sim_with(
        &root,
        vec![entity("Bad", vec![BehaviorComponent::new("lua.broken")])],
    );
    let report = headless::run(&mut sim, &Script::idle(10));
    assert_eq!(report["passed"], false);
    assert_eq!(report["diagnostics"][0]["file"], "scripts/broken.lua");
    assert!(
        report["failures"][0]
            .as_str()
            .unwrap()
            .starts_with("script error: scripts/broken.lua:6:")
    );
    std::fs::remove_dir_all(&root).unwrap();
}

const SANDBOX: &str = r#"
return {
  start = function(self, ctx)
    ctx:set_state("io", type(io))
    ctx:set_state("os", type(os))
    ctx:set_state("require", type(require))
    ctx:set_state("debug", type(debug))
    ctx:set_state("load_file", type(dofile))
    ctx:set_state("loadfile", type(loadfile))
    ctx:set_state("random", math.random(1, 1000000))
  end,
}
"#;

#[test]
fn the_sandbox_has_no_file_access_and_random_numbers_repeat() {
    let root = project("sandbox", &[("sandbox.lua", SANDBOX)]);
    let first = {
        let mut sim = sim_with(
            &root,
            vec![entity("E", vec![BehaviorComponent::new("lua.sandbox")])],
        );
        run(&mut sim, 2);
        assert!(sim.take_diagnostics().is_empty());
        for lib in ["io", "os", "require", "debug", "load_file", "loadfile"] {
            assert_eq!(sim.state()[lib], "nil", "{lib}");
        }
        sim.state()["random"].clone()
    };
    let mut sim = sim_with(
        &root,
        vec![entity("E", vec![BehaviorComponent::new("lua.sandbox")])],
    );
    run(&mut sim, 2);
    assert_eq!(sim.state()["random"], first);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn lua_can_be_switched_off_per_project() {
    let root = project("off", &[("spin.lua", SPIN)]);
    let mut p = Project::open(&root).unwrap();
    p.file.scripting.lua = false;
    p.save().unwrap();
    let sim = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    assert!(sim.registry().get("lua.spin").is_none());
    std::fs::remove_dir_all(&root).unwrap();
}

const STASH: &str = r#"
local kept
return {
  start = function(self, ctx) kept = ctx end,
  fixed_update = function(self, ctx)
    if ctx:tick() == 2 then
      local co = coroutine.wrap(function() return kept:tick() end)
      ctx:set_state("same_table", kept == ctx)
      ctx:set_state("inside", co())
    end
  end,
}
"#;

#[test]
fn ctx_works_inside_coroutines_during_a_callback() {
    let root = project("names", &[("stash.lua", STASH)]);
    let e = entity("E", vec![BehaviorComponent::new("lua.stash")]);
    let mut sim = sim_with(&root, vec![e]);
    run(&mut sim, 3);
    assert!(sim.take_diagnostics().is_empty());
    assert_eq!(sim.state()["inside"], 2);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn every_ctx_method_is_documented() {
    let page = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/wiki/src/guide/lua.md"),
    )
    .unwrap();
    for m in copperframe_game::lua::API_METHODS {
        assert!(
            page.contains(&format!("`ctx:{m}(")),
            "ctx:{m} is not on the Lua wiki page"
        );
    }
}
