//! End-to-end checks of the `copperframe` binary: JSON output, exit codes, atomic
//! batches, revision conflicts and the editor lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use copperframe_assets::fixtures::Fixture;
use serde_json::{Value, json};

fn copperframe(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_copperframe"))
        .arg("--json")
        .args(args)
        .output()
        .expect("run copperframe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "copperframe {args:?} printed non-JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap_or(-1), value)
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("copperframe cli {name} {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn write_batch(dir: &Path, name: &str, batch: Value) -> String {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec_pretty(&batch).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn project_round_trip_through_the_cli() {
    let root = temp("roundtrip");
    let r = root.to_string_lossy().into_owned();
    let (code, out) = copperframe(&["new", &r, "--name", "Room"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = copperframe(&["validate", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["errors"], 0);

    // Register a model.
    let model = root.join("assets/crate.glb");
    std::fs::write(
        &model,
        Fixture::textured_box([0.5; 3], [1.0; 4], None).to_glb(),
    )
    .unwrap();
    let (code, out) = copperframe(&["import", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    let asset = out["assets"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(out["assets"][0]["detail"]["triangles"], 12);

    // Apply a batch that creates a parent and a model child.
    let (_, before) = copperframe(&["inspect", "--project", &r]);
    assert_eq!(before["scene"]["revision"], 0);
    let room = "6f0c3b1e-6a52-4a8e-9a39-6f2b8c9d0a11";
    let batch = write_batch(
        &root,
        "add.json",
        json!({
            "format": "psx_command_batch", "version": 1, "expected_revision": 0,
            "commands": [
                {"op": "create_entity", "id": room, "name": "Room"},
                {"op": "create_entity", "name": "Crate", "parent": room,
                 "transform": {"translation": [1.0, 0.5, -2.0]},
                 "mesh": {"source": {"kind": "model", "asset": asset}}},
                {"op": "set_transform", "id": room, "rotation_euler_deg": [0, 45, 0]}
            ]
        }),
    );
    let (code, out) = copperframe(&["apply", &batch, "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["report"]["revision"], 1);
    let crate_id = out["report"]["commands"][1]["created"][0]
        .as_str()
        .unwrap()
        .to_owned();

    let (_, after) = copperframe(&["inspect", "--project", &r]);
    let entities = after["scene"]["entities"].as_array().unwrap();
    assert_eq!(
        entities.len(),
        6,
        "starter floor, cube, crate and player plus two"
    );
    let c = entities
        .iter()
        .find(|e| e["id"] == crate_id.as_str())
        .unwrap();
    assert_eq!(c["parent"], room);
    assert_eq!(c["mesh"]["source"]["asset"], asset.as_str());
    let (code, out) = copperframe(&["validate", "--project", &r]);
    assert_eq!(code, 0, "{out}");

    // Stale revision: conflict, nothing written.
    let (code, out) = copperframe(&["apply", &batch, "--project", &r]);
    assert_eq!(code, 3, "{out}");
    assert!(out["error"].as_str().unwrap().contains("revision"));

    // A failing command anywhere rejects the whole batch.
    let scene_file = root.join("scenes/main.scene.json");
    let before_bytes = std::fs::read(&scene_file).unwrap();
    let bad = write_batch(
        &root,
        "bad.json",
        json!({
            "format": "psx_command_batch", "version": 1,
            "commands": [
                {"op": "rename", "id": room, "name": "Renamed"},
                {"op": "set_parent", "id": room, "parent": crate_id}
            ]
        }),
    );
    let (code, out) = copperframe(&["apply", &bad, "--project", &r]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(out["failed_command"], 1);
    assert!(out["error"].as_str().unwrap().contains("cycle"));
    assert_eq!(std::fs::read(&scene_file).unwrap(), before_bytes);

    // References to unknown assets are rejected.
    let missing = write_batch(
        &root,
        "missing.json",
        json!({
            "format": "psx_command_batch", "version": 1,
            "commands": [{"op": "set_mesh", "id": room, "mesh": {"source": {"kind": "model", "asset": "00000000-0000-4000-8000-000000000000"}}}]
        }),
    );
    let (code, out) = copperframe(&["apply", &missing, "--project", &r]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(std::fs::read(&scene_file).unwrap(), before_bytes);

    // Dry run validates without writing.
    let rename = write_batch(
        &root,
        "rename.json",
        json!({
            "format": "psx_command_batch", "version": 1, "expected_revision": 1,
            "commands": [{"op": "rename", "id": room, "name": "Hall"}]
        }),
    );
    let (code, out) = copperframe(&["apply", &rename, "--dry-run", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read(&scene_file).unwrap(), before_bytes);

    // While a running editor holds the lock, mutations are refused.
    let project = copperframe_assets::Project::open(&root).unwrap();
    let lock = copperframe_assets::EditorLock::acquire(&project, false).unwrap();
    let (code, out) = copperframe(&["apply", &rename, "--project", &r]);
    assert_eq!(code, 3, "{out}");
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("open in a running editor")
    );
    drop(lock);
    // A lock file left by an editor that crashed is ignored.
    std::fs::write(root.join(".psx/editor.lock"), "12345").unwrap();
    let (code, out) = copperframe(&["apply", &rename, "--project", &r]);
    assert_eq!(code, 0, "{out}");

    // A broken model is an import error; the scene stays valid on disk.
    std::fs::write(&model, b"not a model").unwrap();
    let (code, out) = copperframe(&["validate", "--project", &r]);
    assert_eq!(code, 1, "{out}");
    let diags = out["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["severity"] == "warning"
                && d["message"].as_str().unwrap().contains("fallback"))
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn schema_and_usage_errors() {
    let (code, out) = copperframe(&["schema"]);
    assert_eq!(code, 0);
    assert_eq!(out["formats"]["scene"]["format"], "psx_scene");
    assert!(out["commands"].as_array().unwrap().len() >= 9);
    let status = Command::new(env!("CARGO_BIN_EXE_copperframe"))
        .arg("frobnicate")
        .output()
        .unwrap()
        .status;
    assert_eq!(status.code(), Some(2));
    let (code, out) = copperframe(&["validate", "--project", "/definitely/not/here"]);
    assert_eq!(code, 1);
    assert!(
        out["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no project")
    );
}

#[test]
fn new_writes_a_game_crate_unless_asked_not_to() {
    let root = temp("template");
    let r = root.to_string_lossy().into_owned();
    let (code, out) = copperframe(&["new", &r, "--name", "Spooky Hall"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["game_crate"], true);
    let manifest = std::fs::read_to_string(root.join("game/Cargo.toml")).unwrap();
    assert!(manifest.contains("name = \"spooky_hall\""), "{manifest}");
    assert!(manifest.contains("/crates/copperframe_game'"), "{manifest}");
    assert!(manifest.contains("[workspace]"));
    let lib = std::fs::read_to_string(root.join("game/src/lib.rs")).unwrap();
    assert!(lib.contains("\"Spooky Hall\".into()"));
    assert!(root.join("game/rust-toolchain.toml").is_file());
    assert!(root.join("game/Cargo.lock").is_file());
    // The starter scene is playable: it has a player.
    let (_, inspect) = copperframe(&["inspect", "--project", &r]);
    let kinds: Vec<String> = inspect["scene"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| e["behaviors"].as_array().cloned().unwrap_or_default())
        .filter_map(|b| b["kind"].as_str().map(str::to_owned))
        .collect();
    assert!(kinds.contains(&"psx.player".to_owned()), "{kinds:?}");

    let plain = temp("plain");
    let p = plain.to_string_lossy().into_owned();
    let (code, out) = copperframe(&["new", &p, "--no-game"]);
    assert_eq!(code, 0, "{out}");
    assert!(!plain.join("game").exists());
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&plain).unwrap();
}

#[test]
fn build_refuses_to_overwrite_an_unrelated_folder() {
    let root = temp("build refuse");
    let r = root.to_string_lossy().into_owned();
    assert_eq!(copperframe(&["new", &r, "--no-game"]).0, 0);
    let out_dir = root.join("dist");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(out_dir.join("notes.txt"), "keep me").unwrap();
    let (code, out) = copperframe(&[
        "build",
        "--project",
        &r,
        "--out",
        &out_dir.to_string_lossy(),
        "--replace",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("not an earlier export")
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join("notes.txt")).unwrap(),
        "keep me"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn animated_models_lights_and_shadows_through_the_cli() {
    let root = temp("animated");
    let r = root.to_string_lossy().into_owned();
    let (code, out) = copperframe(&["new", &r, "--name", "Anim", "--no-game"]);
    assert_eq!(code, 0, "{out}");
    std::fs::write(root.join("assets/arm.glb"), Fixture::skinned_arm().to_glb()).unwrap();
    let (code, out) = copperframe(&["import", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    let detail = &out["assets"][0]["detail"];
    assert_eq!(detail["joints"], 2);
    let clips: Vec<&str> = detail["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(clips, ["bend", "lift"]);
    let asset = out["assets"][0]["id"].as_str().unwrap().to_owned();

    let arm = "0d8a7f52-1b1c-4c47-9e0e-2a3f9b6e7c01";
    let batch = write_batch(
        &root,
        "arm.json",
        json!({
            "format": "psx_command_batch", "version": 1,
            "commands": [
                {"op": "create_entity", "id": arm, "name": "Arm",
                 "mesh": {"source": {"kind": "model", "asset": asset}}},
                {"op": "set_animator", "id": arm, "animator": {"clip": "bend", "speed": 0.5}},
                {"op": "set_shadow", "id": arm, "shadow": {"radius": 0.4}},
                {"op": "set_light", "id": arm, "light": {"color": [1.0, 0.8, 0.5], "range": 6.0}}
            ]
        }),
    );
    let (code, out) = copperframe(&["apply", &batch, "--project", &r]);
    assert_eq!(code, 0, "{out}");
    let (_, scene) = copperframe(&["inspect", "--project", &r]);
    let e = scene["scene"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == arm)
        .unwrap()
        .clone();
    assert_eq!(e["animator"]["clip"], "bend");
    assert_eq!(e["light"]["range"], 6.0);
    let (code, out) = copperframe(&["validate", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["warnings"], 0, "{out}");

    // A clip the model does not have is a warning naming the real clips.
    let wrong = write_batch(
        &root,
        "wrong.json",
        json!({
            "format": "psx_command_batch", "version": 1,
            "commands": [{"op": "set_animator", "id": arm, "animator": {"clip": "wave"}}]
        }),
    );
    let (code, out) = copperframe(&["apply", &wrong, "--project", &r]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = copperframe(&["validate", "--project", &r]);
    let message = out["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("wave"))
        .map(|d| d["message"].as_str().unwrap().to_owned())
        .unwrap_or_default();
    assert!(message.contains("bend, lift"), "{out}");
    // Out-of-range values are rejected by the batch.
    let bad = write_batch(
        &root,
        "bad.json",
        json!({
            "format": "psx_command_batch", "version": 1,
            "commands": [{"op": "set_light", "id": arm, "light": {"intensity": 99.0}}]
        }),
    );
    let (code, out) = copperframe(&["apply", &bad, "--project", &r]);
    assert_eq!(code, 1, "{out}");
    std::fs::remove_dir_all(&root).unwrap();
}
