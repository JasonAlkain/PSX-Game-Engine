//! End-to-end checks of the `psx` binary: JSON output, exit codes, atomic
//! batches, revision conflicts and the editor lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use psx_assets::fixtures::Fixture;
use serde_json::{Value, json};

fn psx(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_psx"))
        .arg("--json")
        .args(args)
        .output()
        .expect("run psx");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "psx {args:?} printed non-JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap_or(-1), value)
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("psx cli {name} {}", std::process::id()));
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
    let (code, out) = psx(&["new", &r, "--name", "Room"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = psx(&["validate", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["errors"], 0);

    // Register a model.
    let model = root.join("assets/crate.glb");
    std::fs::write(
        &model,
        Fixture::textured_box([0.5; 3], [1.0; 4], None).to_glb(),
    )
    .unwrap();
    let (code, out) = psx(&["import", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    let asset = out["assets"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(out["assets"][0]["detail"]["triangles"], 12);

    // Apply a batch that creates a parent and a model child.
    let (_, before) = psx(&["inspect", "--project", &r]);
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
    let (code, out) = psx(&["apply", &batch, "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["report"]["revision"], 1);
    let crate_id = out["report"]["commands"][1]["created"][0]
        .as_str()
        .unwrap()
        .to_owned();

    let (_, after) = psx(&["inspect", "--project", &r]);
    let entities = after["scene"]["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 3);
    let c = entities
        .iter()
        .find(|e| e["id"] == crate_id.as_str())
        .unwrap();
    assert_eq!(c["parent"], room);
    assert_eq!(c["mesh"]["source"]["asset"], asset.as_str());
    let (code, out) = psx(&["validate", "--project", &r]);
    assert_eq!(code, 0, "{out}");

    // Stale revision: conflict, nothing written.
    let (code, out) = psx(&["apply", &batch, "--project", &r]);
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
    let (code, out) = psx(&["apply", &bad, "--project", &r]);
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
    let (code, out) = psx(&["apply", &missing, "--project", &r]);
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
    let (code, out) = psx(&["apply", &rename, "--dry-run", "--project", &r]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read(&scene_file).unwrap(), before_bytes);

    // While the editor holds the lock, mutations are refused.
    std::fs::create_dir_all(root.join(".psx")).unwrap();
    std::fs::write(root.join(".psx/editor.lock"), "12345").unwrap();
    let (code, out) = psx(&["apply", &rename, "--project", &r]);
    assert_eq!(code, 3, "{out}");
    assert!(out["error"].as_str().unwrap().contains("open in an editor"));
    std::fs::remove_file(root.join(".psx/editor.lock")).unwrap();
    let (code, _) = psx(&["apply", &rename, "--project", &r]);
    assert_eq!(code, 0);

    // A broken model is an import error; the scene stays valid on disk.
    std::fs::write(&model, b"not a model").unwrap();
    let (code, out) = psx(&["validate", "--project", &r]);
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
    let (code, out) = psx(&["schema"]);
    assert_eq!(code, 0);
    assert_eq!(out["formats"]["scene"]["format"], "psx_scene");
    assert!(out["commands"].as_array().unwrap().len() >= 9);
    let status = Command::new(env!("CARGO_BIN_EXE_psx"))
        .arg("frobnicate")
        .output()
        .unwrap()
        .status;
    assert_eq!(status.code(), Some(2));
    let (code, out) = psx(&["validate", "--project", "/definitely/not/here"]);
    assert_eq!(code, 1);
    assert!(
        out["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no project")
    );
}
