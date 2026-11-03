//! Plays the example project headless with the committed walkthrough
//! script: the same check `psx test-scene` and CI run.

use std::path::{Path, PathBuf};

use psx_assets::project::Project;
use psx_game::Simulation;
use psx_game::audio::Audio;
use psx_game::headless::{self, Script, Step};

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn simulation() -> Simulation {
    let project = Project::open(&project_root()).expect("example project");
    let scene = PathBuf::from(&project.file.startup_scene);
    let mut sim = Simulation::new(project, Box::new(lever_room::LeverRoom), Audio::silent());
    sim.load_scene_file(&scene).expect("startup scene loads");
    sim
}

#[test]
fn walkthrough_script_reaches_the_exit_and_leaves_the_scene_file_alone() {
    let root = project_root();
    let scene_file = root.join("scenes/main.scene.json");
    let before = std::fs::read(&scene_file).unwrap();
    let text = std::fs::read_to_string(root.join("scripts/walkthrough.script.json")).unwrap();
    let script = Script::parse(&text).unwrap();
    let mut sim = simulation();
    let report = headless::run(&mut sim, &script);
    assert_eq!(report["passed"], true, "{report:#}");
    assert!(
        report["warnings"].as_array().unwrap().is_empty(),
        "{report:#}"
    );
    assert_eq!(sim.state().get(lever_room::DOOR_OPEN), Some(&true.into()));
    // The corridor's fixed camera (psx.camera_zone) is active at the exit.
    let cam = &report["camera"]["position"];
    assert!((cam[0].as_f64().unwrap() - 0.7).abs() < 1e-3, "{cam}");
    assert!((cam[1].as_f64().unwrap() - 2.6).abs() < 1e-3, "{cam}");
    sim.shutdown();
    assert_eq!(
        std::fs::read(&scene_file).unwrap(),
        before,
        "play must not write the scene"
    );
}

#[test]
fn the_closed_door_blocks_the_corridor() {
    let mut sim = simulation();
    let script = Script {
        steps: vec![Step {
            ticks: 600,
            hold: vec!["move_forward".into()],
            ..Step::default()
        }],
        ..Script::idle(0)
    };
    let report = headless::run(&mut sim, &script);
    assert_eq!(report["outcome"], serde_json::Value::Null, "{report:#}");
    let z = report["camera"]["position"][2].as_f64().unwrap();
    assert!(z > -4.75, "walked through the door: z = {z}");
}

#[test]
fn schema_lists_the_game_behaviours() {
    let sim = simulation();
    let schema = sim.registry().schema("The Lever Room");
    let kinds: Vec<&str> = schema["behaviors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["kind"].as_str())
        .collect();
    for k in [
        "psx.player",
        "room.door",
        "room.lever",
        "room.inspect",
        "room.exit",
        "room.caretaker",
    ] {
        assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
    }
}

#[test]
fn the_caretaker_waves_then_returns_to_idle() {
    let mut sim = simulation();
    let caretaker = sim
        .find("00000000-0000-4000-8000-000000000080".parse().unwrap())
        .expect("caretaker in the scene");
    let step = |ticks, hold: &[&str], press: &[&str], look: [f32; 2]| Step {
        ticks,
        hold: hold.iter().map(|s| (*s).to_owned()).collect(),
        press: press.iter().map(|s| (*s).to_owned()).collect(),
        look,
    };
    // Turn right towards the caretaker (east of the start), walk up to
    // him, and talk.
    let script = Script {
        steps: vec![
            step(10, &[], &[], [5.11, -0.4]),
            step(90, &["move_forward"], &[], [0.0, 0.0]),
            step(1, &[], &["interact"], [0.0, 0.0]),
            step(20, &[], &[], [0.0, 0.0]),
        ],
        ..Script::idle(0)
    };
    let report = headless::run(&mut sim, &script);
    let events = report["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "custom" && e["name"] == "talked"),
        "{report:#}"
    );
    let clip = |sim: &Simulation| {
        sim.world()
            .get::<&psx_game::animation::Animator>(caretaker)
            .ok()
            .and_then(|a| a.state().map(|(c, _)| c.to_owned()))
    };
    assert_eq!(clip(&sim).as_deref(), Some("wave"));
    // The wave (1.8 s) ends, is reported, and idle fades back in.
    let report = headless::run(&mut sim, &Script::idle(150));
    let events = report["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["event"] == "animation_finished"
            && e["entity"] == "Caretaker"
            && e["clip"] == "wave"),
        "{report:#}"
    );
    assert_eq!(clip(&sim).as_deref(), Some("idle"));
}
