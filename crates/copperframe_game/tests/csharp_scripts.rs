//! C# behaviours: building with `dotnet`, registration and schema, fields,
//! the shared ctx API, messages, game state, prompts, compiler errors and
//! runtime exceptions with file and line, and the project switch.
//!
//! These tests need the .NET SDK. Without it they print a note and pass,
//! unless `COPPERFRAME_REQUIRE_DOTNET=1` is set (CI sets it).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
        "C# test".into()
    }
    fn register(&self, _: &mut Registry) {}
}

fn dotnet_available() -> bool {
    let ok = std::process::Command::new(copperframe_assets::csharp::dotnet_command())
        .arg("--list-sdks")
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty());
    if !ok {
        assert!(
            std::env::var_os("COPPERFRAME_REQUIRE_DOTNET").is_none(),
            "COPPERFRAME_REQUIRE_DOTNET is set but the .NET SDK was not found"
        );
        eprintln!("skipping: the .NET SDK is not installed");
    }
    ok
}

const SPINNER: &str = r#"
using Copperframe;

[Behaviour("Turns about Y", Folder = "Motion")]
public class Spinner : Behaviour
{
    [Help("degrees per second")] public float Speed = 45;
    public bool Enabled = true;
    public string Label = "spinner";
    public EntityRef Partner;
    [Hide] public int Internal = 3;

    public override void FixedUpdate(Ctx ctx)
    {
        if (Enabled) ctx.Rotate(Entity, Speed * ctx.Dt());
    }
}
"#;

const LEVER_AND_DOOR: &str = r#"
using System.Numerics;
using Copperframe;

public class Lever : Behaviour
{
    public EntityRef Door;

    public override void Start(Ctx ctx) => Prompt = "Pull lever";

    public override void FixedUpdate(Ctx ctx)
    {
        if (ctx.Tick() == 1 && ctx.Find(Door) is Entity door)
        {
            ctx.Send(door, "open");
            ctx.SetState("lever_pulled", true);
            Prompt = null;
        }
    }
}

public class Door : Behaviour
{
    public override void OnMessage(Ctx ctx, string message, Entity? from)
    {
        if (message != "open" || from is not Entity f || ctx.Name(f) != "Lever") return;
        var p = ctx.Position(Entity);
        ctx.SetPosition(Entity, p + new Vector3(0, 2, 0));
        ctx.Event("door_opened", ctx.Name(Entity));
        ctx.SetState("opened_at", ctx.Tick());
        ctx.SetState("label", "open");
    }
}
"#;

// Line numbers matter: the exception is on line 9, the bad ctx call on 17.
const BROKEN: &str = r#"using Copperframe;

public class Broken : Behaviour
{
    public override void FixedUpdate(Ctx ctx)
    {
        if (ctx.Tick() == 3) {
            string? s = null;
            ctx.SetState("len", s!.Length);
        }
        ctx.SetState("still_running", ctx.Tick());
    }
}

public class BadArg : Behaviour
{
    public override void Start(Ctx ctx) => ctx.SetPosition(default, System.Numerics.Vector3.Zero);
}

public class Counter : Behaviour
{
    public override void FixedUpdate(Ctx ctx)
    {
        var n = ctx.GetState("counter") is long c ? c : 0;
        ctx.SetState("counter", n + 1);
    }
}
"#;

/// A project with C# on and `scripts` under `scripts/`.
fn project(name: &str, scripts: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cf_cs_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut p = Project::create(&root, name).unwrap();
    p.file.scripting.csharp = true;
    p.save().unwrap();
    for (file, text) in scripts {
        let path = root.join("scripts").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

/// One project shared by the tests that only read it, so it is built once.
fn shared() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        // The editor's New script template, which must compile as is.
        let template = copperframe_assets::csharp::cs_template("Tools/FromTemplate.cs");
        project(
            "shared",
            &[
                ("Spinner.cs", SPINNER),
                ("Puzzle/LeverAndDoor.cs", LEVER_AND_DOOR),
                ("Broken.cs", BROKEN),
                ("Tools/FromTemplate.cs", &template),
            ],
        )
    })
}

fn entity(name: &str, behaviors: Vec<BehaviorComponent>) -> SceneEntity {
    let mut e = SceneEntity::new(name);
    e.behaviors = behaviors;
    e
}

fn sim_with(root: &Path, scene_name: &str, entities: Vec<SceneEntity>) -> Simulation {
    let mut scene = Scene::new("C#");
    scene.entities = entities;
    let path = root.join(format!("scenes/{scene_name}.scene.json"));
    copperframe_scene::save_scene(&scene, &path).unwrap();
    let mut sim = Simulation::new(
        Project::open(root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    let load = sim.take_diagnostics();
    assert!(load.is_empty(), "{load:?}");
    sim.load_scene_file(&path).unwrap();
    sim
}

fn run(sim: &mut Simulation, ticks: u32) {
    for _ in 0..ticks {
        sim.tick(&ActionState::default());
    }
}

#[test]
fn classes_register_with_fields_and_folders_in_the_schema() {
    if !dotnet_available() {
        return;
    }
    let sim = Simulation::new(
        Project::open(shared()).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    let schema = sim.registry().schema("t");
    let kinds = schema["behaviors"].as_array().unwrap();
    let spin = kinds
        .iter()
        .find(|b| b["kind"] == "cs.spinner")
        .unwrap_or_else(|| panic!("{schema:#}"));
    assert_eq!(spin["description"], "Turns about Y");
    assert_eq!(spin["folder"], "Scripts/Motion");
    let fields: Vec<(&str, &str)> = spin["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        fields,
        [
            ("speed", "number"),
            ("enabled", "bool"),
            ("label", "text"),
            ("partner", "entity")
        ]
    );
    assert_eq!(spin["fields"][0]["default"], 45.0);
    assert_eq!(spin["fields"][0]["help"], "degrees per second");
    let template = kinds
        .iter()
        .find(|b| b["kind"] == "cs.from_template")
        .unwrap_or_else(|| panic!("{schema:#}"));
    assert_eq!(template["folder"], "Scripts/Tools");
    assert_eq!(template["fields"][0]["name"], "speed");
    for k in [
        "cs.lever",
        "cs.door",
        "cs.broken",
        "cs.bad_arg",
        "cs.counter",
    ] {
        assert!(kinds.iter().any(|b| b["kind"] == k), "{k} missing");
    }
    assert!(schema.get("problems").is_none(), "{schema:#}");
}

#[test]
fn a_class_moves_its_entity_with_scene_parameters() {
    if !dotnet_available() {
        return;
    }
    let spinner = BehaviorComponent::new("cs.spinner").with("speed", 90.0);
    let e = entity("Spinner", vec![spinner]);
    let id = e.id;
    let mut sim = sim_with(shared(), "spin", vec![e]);
    run(&mut sim, 60);
    assert!(sim.take_diagnostics().is_empty());
    let ent = sim.find(id).unwrap();
    let rot = sim
        .world()
        .get::<&copperframe_game::sim::Local>(ent)
        .unwrap()
        .0
        .rotation;
    let (yaw, _, _) = rot.to_euler(EulerRot::YXZ);
    assert!(
        (yaw.to_degrees() - 90.0).abs() < 0.5,
        "yaw {}",
        yaw.to_degrees()
    );
}

#[test]
fn classes_message_each_other_log_events_keep_state_and_prompt() {
    if !dotnet_available() {
        return;
    }
    let mut door = entity("Door", vec![BehaviorComponent::new("cs.door")]);
    door.transform = Transform::from_translation(Vec3::new(0.0, 1.0, 0.0));
    let door_id = door.id;
    let lever = entity(
        "Lever",
        vec![BehaviorComponent::new("cs.lever").with("door", door_id.to_string())],
    );
    let lever_id = lever.id;
    let mut sim = sim_with(shared(), "msg", vec![lever, door]);
    run(&mut sim, 1);
    let l = sim.find(lever_id).unwrap();
    assert_eq!(sim.interaction_prompt(l).as_deref(), Some("Pull lever"));
    run(&mut sim, 3);
    assert!(sim.take_diagnostics().is_empty());
    assert_eq!(sim.interaction_prompt(l), None);
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
    assert_eq!(sim.state()["label"], "open");
    assert!(sim.take_events().iter().any(|t| matches!(
        &t.event,
        GameEvent::Custom { name, detail } if name == "door_opened" && detail == "Door"
    )));
}

#[test]
fn an_exception_switches_off_only_that_behaviour_with_its_line() {
    if !dotnet_available() {
        return;
    }
    let mut sim = sim_with(
        shared(),
        "err",
        vec![
            entity("Bad", vec![BehaviorComponent::new("cs.broken")]),
            entity("Arg", vec![BehaviorComponent::new("cs.bad_arg")]),
            entity("Good", vec![BehaviorComponent::new("cs.counter")]),
        ],
    );
    run(&mut sim, 20);
    let d = sim.take_diagnostics();
    assert_eq!(d.len(), 2, "{d:?}");
    let arg = d.iter().find(|d| d.kind == "cs.bad_arg").unwrap();
    assert_eq!(arg.file, "scripts/Broken.cs");
    assert_eq!(arg.line, Some(17), "{arg:?}");
    assert_eq!(arg.callback.as_deref(), Some("Start"));
    assert!(
        arg.message.contains("ctx.set_position") && arg.message.contains("not an entity"),
        "{}",
        arg.message
    );
    let bad = d.iter().find(|d| d.kind == "cs.broken").unwrap();
    assert_eq!(bad.line, Some(9), "{bad:?}");
    assert_eq!(bad.entity.as_deref(), Some("Bad"));
    assert_eq!(bad.callback.as_deref(), Some("FixedUpdate"));
    assert_eq!(bad.tick, Some(3));
    assert!(
        bad.message.contains("NullReferenceException"),
        "{}",
        bad.message
    );
    assert_eq!(sim.state()["still_running"], 2, "off after the error");
    assert_eq!(sim.state()["counter"], 20, "the other script kept running");
    let report = headless::run(&mut sim, &Script::idle(1));
    assert_eq!(
        report["passed"], true,
        "already reported diagnostics are not repeated"
    );
}

#[test]
fn compiler_errors_are_reported_with_file_and_line_and_nothing_registers() {
    if !dotnet_available() {
        return;
    }
    let root = project(
        "compile",
        &[(
            "Typo.cs",
            "using Copperframe;\npublic class Typo : Behaviour {\n  public override void Start(Ctx ctx) { int x = \"a\"; }\n}\n",
        )],
    );
    let sim = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    assert!(sim.registry().get("cs.typo").is_none());
    let schema = sim.registry().schema("t");
    assert!(
        schema["behaviors"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["kind"] != "cs.typo")
    );
    let mut sim = sim;
    let d = sim.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].file, "scripts/Typo.cs");
    assert_eq!(d[0].line, Some(3));
    assert!(d[0].message.starts_with("CS0029"), "{}", d[0].message);
    let mut fresh = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    let report = headless::run(&mut fresh, &Script::idle(1));
    assert_eq!(report["passed"], false, "{report}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn csharp_is_off_unless_the_project_turns_it_on() {
    // Needs no .NET: nothing is built when C# is off.
    let root = std::env::temp_dir().join(format!("cf_cs_off_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Project::create(&root, "off").unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/Spinner.cs"), SPINNER).unwrap();
    let mut sim = Simulation::new(
        Project::open(&root).unwrap(),
        Box::new(NoGame),
        Audio::silent(),
    );
    assert!(sim.registry().get("cs.spinner").is_none());
    assert!(sim.take_diagnostics().is_empty());
    assert!(!root.join(".psx/csharp").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn every_ctx_method_has_a_csharp_binding() {
    let api = include_str!("../../copperframe_assets/csharp/Copperframe.cs");
    for m in copperframe_game::script_api::METHODS {
        assert!(
            api.contains(&format!("\"{m}\"u8")),
            "ctx method `{m}` has no binding in Copperframe.cs"
        );
    }
}
