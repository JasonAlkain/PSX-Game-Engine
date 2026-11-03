//! `copperframe`: project tools without the GUI.
//!
//! Every subcommand prints a human-readable report, or with `--json` one
//! JSON object on stdout: `{"ok": bool, "command": "...", ...}`. Errors go
//! to the `error` field (JSON) or stderr (text).
//!
//! Exit codes:
//! - 0: success
//! - 1: validation found errors, a batch was rejected, or an import failed
//! - 2: usage error (bad arguments)
//! - 3: conflict: the project is open in the editor, or the scene revision
//!   does not match the batch's `expected_revision`
//! - 4: I/O or file-format error (missing project, unreadable file)
//!
//! Scene mutations (`apply`) refuse to run while the editor holds the
//! project lock, so unsaved editor work is never overwritten.
//!
//! `test-scene` and `build` run the project's game: its `game/` crate (built
//! with cargo) or, without one, the stock `copperframe_player` next to this tool.

mod template;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};

use clap::{Parser, Subcommand};
use copperframe_assets::export::{ExportOptions, check_output_folder, export, verify_export};
use copperframe_assets::game::GameTarget;
use copperframe_assets::project::{AssetDatabase, ImportedAsset, Project, ProjectError};
use copperframe_assets::validate_project;
use copperframe_scene::batch::{BATCH_FORMAT, BATCH_VERSION, BatchError, parse_batch};
use copperframe_scene::file::{SCENE_FORMAT, SCENE_VERSION};
use copperframe_scene::{Command, apply_batch, load_scene, save_scene};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "copperframe",
    version,
    about = "Copperframe Engine project tools"
)]
struct Cli {
    /// Print one JSON object instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a project (project.json, assets/, scenes/main.scene.json).
    New {
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Do not create the Rust game crate (`game/`); the project then
        /// plays in the stock player with engine behaviours only.
        #[arg(long)]
        no_game: bool,
    },
    /// Describe file formats, components and batch commands. With
    /// --project, also the behaviour kinds that project's game registers.
    Schema {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Check the project file, asset metadata, every import and every scene.
    Validate {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Register (create .meta.json) and test-import an asset under assets/.
    /// With no file, registers every unregistered asset.
    Import {
        file: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Print a scene with stable IDs and serialized component state.
    Inspect {
        /// Scene file (default: the project's startup scene).
        scene: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Apply a command batch to a scene, all or nothing.
    Apply {
        /// Batch file (`-` reads stdin).
        batch: PathBuf,
        /// Scene file (default: the project's startup scene).
        #[arg(long)]
        scene: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Validate and report without writing the scene.
        #[arg(long)]
        dry_run: bool,
    },
    /// Play a scene headless with the project's game and report what
    /// happened (fails if a script expectation fails).
    TestScene {
        /// Scene file (default: the project's startup scene).
        scene: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Input script (`psx_script` JSON) with optional expectations.
        #[arg(long)]
        script: Option<PathBuf>,
        /// Ticks to run without a script (60 per second).
        #[arg(long, default_value_t = 300)]
        ticks: u64,
    },
    /// Measure simulation tick and frame times: builds the game in release,
    /// then renders the scene offscreen for --frames frames in the
    /// authored, PS1 and PS2 styles. Needs a graphics adapter.
    Bench {
        /// Scene file (default: the project's startup scene).
        scene: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Input script (`psx_script` JSON); idle without one.
        #[arg(long)]
        script: Option<PathBuf>,
        /// Measured frames per style.
        #[arg(long, default_value_t = 300)]
        frames: u32,
        /// Output size, WIDTHxHEIGHT.
        #[arg(long, default_value = "1280x720")]
        size: String,
    },
    /// Export the game for this platform: release executable, project data,
    /// third-party notices. The export is then run headless from another
    /// folder to check it starts.
    Build {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Output folder (created; must be empty or an earlier export).
        #[arg(long)]
        out: PathBuf,
        /// Overwrite an earlier export in --out.
        #[arg(long)]
        replace: bool,
        /// Script for the post-export check (default: idle run).
        #[arg(long)]
        script: Option<PathBuf>,
        /// Skip the post-export check.
        #[arg(long)]
        no_verify: bool,
    },
}

/// A failed command: exit code plus message.
struct Failure {
    code: u8,
    message: String,
    extra: Value,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            extra: Value::Null,
        }
    }
}

impl From<ProjectError> for Failure {
    fn from(e: ProjectError) -> Self {
        let code = if matches!(e, ProjectError::Locked { .. }) {
            3
        } else {
            4
        };
        Failure::new(code, e.to_string())
    }
}

/// Successful result: JSON payload and text rendering.
struct Success {
    payload: Value,
    text: String,
}

fn main() -> ExitCode {
    use copperframe_core::report;
    report::install(
        "copperframe",
        report::engine_data_dir(),
        Some(report::REPORT_EMAIL),
    );
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 2 } else { 0 });
        }
    };
    let name = match &cli.command {
        Cmd::New { .. } => "new",
        Cmd::Schema { .. } => "schema",
        Cmd::TestScene { .. } => "test-scene",
        Cmd::Build { .. } => "build",
        Cmd::Bench { .. } => "bench",
        Cmd::Validate { .. } => "validate",
        Cmd::Import { .. } => "import",
        Cmd::Inspect { .. } => "inspect",
        Cmd::Apply { .. } => "apply",
    };
    let result = match cli.command {
        Cmd::New { dir, name, no_game } => new(&dir, name, no_game),
        Cmd::Schema { project } => schema(project.as_deref()),
        Cmd::TestScene {
            scene,
            project,
            script,
            ticks,
        } => test_scene(&project, scene.as_deref(), script.as_deref(), ticks),
        Cmd::Bench {
            scene,
            project,
            script,
            frames,
            size,
        } => bench(&project, scene.as_deref(), script.as_deref(), frames, &size),
        Cmd::Build {
            project,
            out,
            replace,
            script,
            no_verify,
        } => build(
            &project,
            &out,
            replace,
            script.as_deref(),
            no_verify,
            cli.json,
        ),
        Cmd::Validate { project } => validate(&project),
        Cmd::Import { file, project } => import(&project, file.as_deref()),
        Cmd::Inspect { scene, project } => inspect(&project, scene.as_deref()),
        Cmd::Apply {
            batch,
            scene,
            project,
            dry_run,
        } => apply(&project, scene.as_deref(), &batch, dry_run),
    };
    match result {
        Ok(ok) => {
            if cli.json {
                let mut out = json!({"ok": true, "command": name});
                merge(&mut out, ok.payload);
                println!("{out}");
            } else {
                print!("{}", ok.text);
            }
            ExitCode::SUCCESS
        }
        Err(fail) => {
            if cli.json {
                let mut out = json!({"ok": false, "command": name, "error": fail.message});
                merge(&mut out, fail.extra);
                println!("{out}");
            } else {
                eprintln!("error: {}", fail.message);
            }
            ExitCode::from(fail.code)
        }
    }
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(into), Value::Object(from)) = (into.as_object_mut(), from) {
        into.extend(from);
    }
}

fn new(dir: &Path, name: Option<String>, no_game: bool) -> Result<Success, Failure> {
    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    });
    let project = Project::create(dir, &name)?;
    let scene = project.startup_scene_path();
    let game = if no_game {
        "none (plays in the stock copperframe_player)".to_owned()
    } else {
        match template::engine_root() {
            Some(engine) => {
                template::write_game_crate(dir, &name, &engine).map_err(|e| Failure::new(4, e))?;
                format!(
                    "game/ (Rust crate using the engine at {})",
                    engine.display()
                )
            }
            None => {
                return Err(Failure::new(
                    4,
                    format!(
                        "created the project, but not its game crate: the engine source this \
                         tool was built from is gone. Use --no-game, or rebuild copperframe from an \
                         engine checkout. Project: {}",
                        dir.display()
                    ),
                ));
            }
        }
    };
    Ok(Success {
        payload: json!({
            "project": dir.display().to_string(),
            "name": name,
            "startup_scene": project.file.startup_scene,
            "game_crate": !no_game,
        }),
        text: format!(
            "created project \"{name}\" in {}\n  startup scene: {}\n  gameplay code: {game}\n",
            dir.display(),
            scene.display()
        ),
    })
}

fn schema(project: Option<&Path>) -> Result<Success, Failure> {
    let ops: Vec<Value> = Command::OPS
        .iter()
        .map(|(op, args)| json!({"op": op, "fields": args}))
        .collect();
    let payload = json!({
        "formats": {
            "project": {"format": copperframe_assets::project::PROJECT_FORMAT, "version": copperframe_assets::project::PROJECT_VERSION},
            "scene": {"format": SCENE_FORMAT, "version": SCENE_VERSION},
            "command_batch": {"format": BATCH_FORMAT, "version": BATCH_VERSION},
            "asset_meta": {"format": copperframe_assets::meta::META_FORMAT, "version": copperframe_assets::meta::META_VERSION},
            "style": {"format": copperframe_core::style::STYLE_FORMAT, "version": copperframe_core::style::STYLE_VERSION},
        },
        "importer_version": copperframe_assets::IMPORTER_VERSION,
        "asset_kinds": {
            "gltf": [".gltf", ".glb"],
            "texture": [".png", ".jpg", ".jpeg"],
            "sound": [".wav", ".ogg"],
        },
        "components": {
            "transform": {
                "translation": "[x, y, z] meters, relative to the parent",
                "rotation": "unit quaternion [x, y, z, w]",
                "scale": "[x, y, z], each |axis| >= 0.0001",
            },
            "mesh": {
                "source": "{kind: primitive, shape: cube|plane|sphere|quad, texture?: texture asset id} | {kind: model, asset: model asset id}",
                "material": "{tint?: [r,g,b,a] linear, alpha?: {mode: opaque|blend} | {mode: cutout, threshold}, lit?, flat_shading?, allow_affine?, double_sided?}",
            },
            "collider": {
                "shape": "{kind: box, size: [x,y,z]} | {kind: sphere, radius} | {kind: capsule, radius, height} | {kind: mesh} (static only); sizes are in local space and follow the entity's scale",
                "offset": "[x, y, z] local, default 0",
                "body": "static (default) | dynamic | kinematic; dynamic and kinematic bodies must be root entities",
                "sensor": "bool: reports overlaps instead of colliding",
                "layer": "0..15 (default 0)",
                "mask": "u16 bitmask of layers it collides with (default all)",
                "mass": "kg, dynamic bodies (default 1)",
                "friction": "0..=10 (default 0.5)",
            },
            "audio": {
                "sound": "sound asset id",
                "bus": "effects (default) | music",
                "volume_db": "default 0",
                "looping": "bool", "autoplay": "bool: start when the scene starts",
                "spatial": "bool (default true): positioned at the entity",
                "min_distance": "full volume within (m, default 1)",
                "max_distance": "silent beyond (m, default 20)",
            },
            "light": {
                "color": "[r, g, b] linear, each 0..=1 (default white)",
                "intensity": "0..=16 (default 1)",
                "range": "meters 0.05..=100 (default 4); falls off smoothly to zero at range",
                "note": "point light at the entity; the style's shading.local_lights must be on; at most 8 nearest the camera are used",
            },
            "animator": {
                "clip": "clip name in the entity's skinned model (`copperframe import` lists clips)",
                "speed": "-10..=10 (default 1; negative plays backwards)",
                "looping": "bool (default true)",
                "autoplay": "bool (default true); false holds the clip's first frame until gameplay code plays a clip",
                "note": "needs a model mesh with a skin; gameplay code switches clips with Ctx::play_animation (crossfades)",
            },
            "shadow": {
                "radius": "local units 0.05..=20 (default 0.5)",
                "offset": "[x, y, z] local, the caster's bottom (default 0)",
                "note": "blob shadow on upward-facing lit surfaces below; the style's shadows.enabled must be on",
            },
            "behaviors": "[{kind, params: {name: bool|number|string}}]; kinds come from the game (`copperframe schema --project` lists them); `psx.*` kinds are built into the engine",
        },
        "conventions": {
            "coordinates": "right-handed, +Y up, meters, camera looks along -Z",
            "rotation_euler_deg": "[x, y, z] degrees; applied about Y, then X, then Z (intrinsic)",
            "ids": "UUID strings; entity ids may be chosen by the caller in create_entity",
        },
        "commands": ops,
        "scripts": {"format": psx_script_format(), "version": 1,
            "steps": "[{ticks, hold: [action], press: [action] (first tick), look: [x°, y°] per tick}]",
            "expect": "{finished?: bool, events?: [partial event objects]}"},
    });
    let mut payload = payload;
    let mut behaviours_text = String::new();
    if let Some(root) = project {
        let project = Project::open(root)?;
        let target = GameTarget::for_project(&project).map_err(|e| Failure::new(4, e))?;
        let game = target.schema().map_err(|e| Failure::new(1, e))?;
        if let Err(e) = copperframe_assets::game::write_schema_cache(&project, &game) {
            eprintln!("warning: {e}");
        }
        for b in game["behaviors"].as_array().into_iter().flatten() {
            behaviours_text.push_str(&format!(
                "  {:<18} {}\n",
                b["kind"].as_str().unwrap_or("?"),
                b["description"].as_str().unwrap_or("")
            ));
        }
        payload["game"] = game;
    }
    let mut text = String::from("formats:\n");
    for (k, v) in payload["formats"].as_object().into_iter().flatten() {
        text.push_str(&format!("  {k}: {} v{}\n", v["format"], v["version"]));
    }
    text.push_str("batch commands:\n");
    for (op, args) in Command::OPS {
        text.push_str(&format!("  {op:<18} {args}\n"));
    }
    if !behaviours_text.is_empty() {
        text.push_str("behaviour kinds:\n");
        text.push_str(&behaviours_text);
    }
    Ok(Success { payload, text })
}

fn psx_script_format() -> &'static str {
    "psx_script"
}

fn absolute(p: &Path) -> Result<PathBuf, Failure> {
    std::path::absolute(p).map_err(|e| Failure::new(4, format!("{}: {e}", p.display())))
}

fn test_scene(
    root: &Path,
    scene: Option<&Path>,
    script: Option<&Path>,
    ticks: u64,
) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    let target = GameTarget::for_project(&project).map_err(|e| Failure::new(4, e))?;
    let mut args: Vec<OsString> = vec![
        "--project".into(),
        absolute(root)?.into(),
        "--headless".into(),
    ];
    if let Some(scene) = scene {
        // Like the other commands, a relative scene path is relative to the
        // current folder.
        let path = if scene.is_absolute() {
            scene.to_path_buf()
        } else {
            absolute(scene)?
        };
        args.extend(["--scene".into(), path.into()]);
    }
    match script {
        Some(s) => args.extend(["--script".into(), absolute(s)?.into()]),
        None => args.extend(["--ticks".into(), ticks.to_string().into()]),
    }
    let out = target
        .command(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Failure::new(4, format!("could not run {}: {e}", target.describe())))?;
    let report: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(20).collect();
        Failure::new(
            4,
            format!(
                "{} did not produce a report ({}):\n{}",
                target.describe(),
                out.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ),
        )
    })?;
    let mut text = format!(
        "{} ran {} ticks ({:.1} s) of {}\n",
        report["game"].as_str().unwrap_or("game"),
        report["ticks"],
        report["seconds"].as_f64().unwrap_or(0.0),
        report["scene"].as_str().unwrap_or("?")
    );
    for e in report["events"].as_array().into_iter().flatten() {
        text.push_str(&format!("  {e}\n"));
    }
    for w in report["warnings"].as_array().into_iter().flatten() {
        text.push_str(&format!("  warning: {}\n", w.as_str().unwrap_or_default()));
    }
    if let Some(o) = report["outcome"].as_object() {
        text.push_str(&format!("outcome: {}\n", Value::Object(o.clone())));
    }
    if report["passed"] == true {
        text.push_str("passed\n");
        Ok(Success {
            payload: json!({"report": report}),
            text,
        })
    } else {
        let failures: Vec<String> = report["failures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str().map(str::to_owned))
            .collect();
        Err(Failure {
            code: 1,
            message: format!("{text}failed:\n  {}", failures.join("\n  ")),
            extra: json!({"report": report}),
        })
    }
}

fn bench(
    root: &Path,
    scene: Option<&Path>,
    script: Option<&Path>,
    frames: u32,
    size: &str,
) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    let target = GameTarget::for_project(&project).map_err(|e| Failure::new(4, e))?;
    eprintln!("building the release executable…");
    let exe = target
        .build_release(&mut |line| eprintln!("{line}"))
        .map_err(|e| Failure::new(1, e))?;
    let mut args: Vec<OsString> = vec![
        "--project".into(),
        absolute(root)?.into(),
        "--bench".into(),
        "--frames".into(),
        frames.to_string().into(),
        "--bench-size".into(),
        size.into(),
    ];
    if let Some(scene) = scene {
        let path = if scene.is_absolute() {
            scene.to_path_buf()
        } else {
            absolute(scene)?
        };
        args.extend(["--scene".into(), path.into()]);
    }
    if let Some(s) = script {
        args.extend(["--script".into(), absolute(s)?.into()]);
    }
    let out = std::process::Command::new(&exe)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Failure::new(4, format!("could not run {}: {e}", exe.display())))?;
    let report: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(20).collect();
        Failure::new(
            1,
            format!(
                "the benchmark did not produce a report ({}):\n{}",
                out.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ),
        )
    })?;
    let mut text = format!(
        "{} on {} ({}, {}), {} build, {}x{} output, {} frames per style\n",
        report["scene"].as_str().unwrap_or("?"),
        report["adapter"]["name"].as_str().unwrap_or("?"),
        report["adapter"]["backend"].as_str().unwrap_or("?"),
        report["adapter"]["device_type"].as_str().unwrap_or("?"),
        report["build"].as_str().unwrap_or("?"),
        report["output"][0],
        report["output"][1],
        report["frames_per_style"],
    );
    for s in report["styles"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "  {:<6} {}x{} internal, {} draws, {} triangles: frame mean {} ms, p95 {} ms, max {} ms; tick mean {} ms\n",
            s["style"].as_str().unwrap_or("?"),
            s["internal"][0],
            s["internal"][1],
            s["draws"],
            s["triangles"],
            s["frame"]["mean_ms"],
            s["frame"]["p95_ms"],
            s["frame"]["max_ms"],
            s["tick"]["mean_ms"],
        ));
    }
    for w in report["warnings"].as_array().into_iter().flatten() {
        text.push_str(&format!("  warning: {}\n", w.as_str().unwrap_or_default()));
    }
    Ok(Success {
        payload: json!({"report": report}),
        text,
    })
}

fn build(
    root: &Path,
    out: &Path,
    replace: bool,
    script: Option<&Path>,
    no_verify: bool,
    json_output: bool,
) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    // Refuse an unusable output folder before finding or building the game.
    check_output_folder(out, replace).map_err(|e| Failure::new(1, e))?;
    let target = GameTarget::for_project(&project).map_err(|e| Failure::new(4, e))?;
    let notices_manifest = match &target {
        GameTarget::Crate { manifest } => manifest.clone(),
        GameTarget::Executable { .. } => template::engine_root()
            .map(|r| r.join("apps/copperframe_player/Cargo.toml"))
            .ok_or_else(|| {
                Failure::new(
                    4,
                    "cannot list the stock player's third-party licences: the engine source \
                     this tool was built from is gone",
                )
            })?,
    };
    let options = ExportOptions {
        out: out.to_path_buf(),
        replace,
        notices_manifest,
    };
    // Progress goes to stderr so --json output stays one object.
    let mut log = |line: &str| {
        if !json_output {
            eprintln!("{line}");
        }
    };
    let report = export(&project, &target, &options, &mut log).map_err(|e| Failure::new(1, e))?;
    let mut text = format!(
        "exported {} to {}\n  {} files, {:.1} MiB, {} third-party crates in NOTICES.txt\n",
        project.file.name,
        report.out.display(),
        report.files,
        report.bytes as f64 / (1024.0 * 1024.0),
        report.crates
    );
    let verified = if no_verify {
        text.push_str("  post-export check skipped (--no-verify)\n");
        Value::Null
    } else {
        let run = verify_export(&report.executable, script, 120).map_err(|e| Failure::new(1, e))?;
        text.push_str(&format!(
            "  checked: the exported game ran {} ticks headless from another folder\n",
            run["ticks"]
        ));
        run
    };
    Ok(Success {
        payload: json!({
            "out": report.out.display().to_string(),
            "executable": report.executable.display().to_string(),
            "files": report.files,
            "bytes": report.bytes,
            "crates": report.crates,
            "verified": verified,
        }),
        text,
    })
}

fn validate(root: &Path) -> Result<Success, Failure> {
    let report = validate_project(root);
    let mut text = String::new();
    for d in &report.diagnostics {
        text.push_str(&format!("{d}\n"));
    }
    text.push_str(&format!(
        "{} assets, {} scenes: {} errors, {} warnings\n",
        report.assets.len(),
        report.scenes.len(),
        report.errors,
        report.warnings
    ));
    let payload = serde_json::to_value(&report).unwrap_or(Value::Null);
    if report.ok() {
        Ok(Success { payload, text })
    } else {
        Err(Failure {
            code: 1,
            message: format!("{} errors\n{text}", report.errors),
            extra: payload,
        })
    }
}

fn import(root: &Path, file: Option<&Path>) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    let files = match file {
        Some(f) => {
            AssetDatabase::register(&project, f)?;
            vec![
                f.canonicalize()
                    .map_err(|e| Failure::new(4, format!("{}: {e}", f.display())))?,
            ]
        }
        None => AssetDatabase::register_all(&project)?,
    };
    let db = AssetDatabase::scan(&project);
    let mut results = Vec::new();
    let mut text = String::new();
    let mut failed = false;
    for f in &files {
        let Some(entry) = db.entries.iter().find(|e| {
            e.source
                .canonicalize()
                .is_ok_and(|c| c == f.canonicalize().unwrap_or_else(|_| f.clone()))
        }) else {
            continue;
        };
        match AssetDatabase::import(&project, entry) {
            Ok(asset) => {
                let detail = match &asset {
                    ImportedAsset::Model(m) => json!({
                        "parts": m.parts.len(),
                        "triangles": m.triangle_count(),
                        "materials": m.materials.len(),
                        "textures": m.textures.len(),
                        "joints": m.skeleton.as_ref().map(|s| s.joints.len()),
                        "clips": m.clips.iter().map(|c| json!({
                            "name": c.name, "duration_secs": c.duration,
                        })).collect::<Vec<_>>(),
                    }),
                    ImportedAsset::Texture(t) => json!({"width": t.width, "height": t.height}),
                    ImportedAsset::Audio(a) => json!({
                        "format": a.format,
                        "channels": a.channels,
                        "sample_rate": a.sample_rate,
                        "duration_secs": a.duration_secs,
                    }),
                };
                text.push_str(&format!(
                    "{} {} ({}) {detail}\n",
                    entry.id,
                    entry.relative,
                    entry.kind.label()
                ));
                for w in asset.warnings() {
                    text.push_str(&format!("  warning: {w}\n"));
                }
                results.push(json!({
                    "id": entry.id, "path": entry.relative, "kind": entry.kind,
                    "ok": true, "detail": detail, "warnings": asset.warnings(),
                }));
            }
            Err(e) => {
                failed = true;
                text.push_str(&format!("{} {}: error: {e}\n", entry.id, entry.relative));
                results.push(json!({
                    "id": entry.id, "path": entry.relative, "kind": entry.kind,
                    "ok": false, "error": e.to_string(),
                }));
            }
        }
    }
    if files.is_empty() {
        text.push_str("nothing to register\n");
    }
    let payload = json!({"assets": results});
    if failed {
        Err(Failure {
            code: 1,
            message: format!("import failed\n{text}"),
            extra: payload,
        })
    } else {
        Ok(Success { payload, text })
    }
}

fn scene_path(project: &Project, scene: Option<&Path>) -> PathBuf {
    scene
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project.startup_scene_path())
}

fn inspect(root: &Path, scene: Option<&Path>) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    let path = scene_path(&project, scene);
    let scene = load_scene(&path).map_err(|e| Failure::new(4, e.to_string()))?;
    let db = AssetDatabase::scan(&project);
    let mut text = format!(
        "scene \"{}\" {} revision {} ({} entities)\n",
        scene.name,
        scene.id,
        scene.revision,
        scene.entities.len()
    );
    fn tree(
        scene: &copperframe_scene::Scene,
        db: &AssetDatabase,
        parent: Option<copperframe_core::EntityId>,
        depth: usize,
        out: &mut String,
    ) {
        for e in scene.children(parent) {
            let t = &e.transform;
            let mesh = match e.mesh.as_ref().map(|m| &m.source) {
                Some(copperframe_scene::MeshSource::Primitive { shape, .. }) => {
                    format!(" [{}]", shape.label())
                }
                Some(copperframe_scene::MeshSource::Model { asset }) => format!(
                    " [model {}]",
                    db.get(*asset)
                        .map_or("MISSING".to_owned(), |a| a.relative.clone())
                ),
                None => String::new(),
            };
            let mut extras = String::new();
            if let Some(a) = &e.animator {
                extras.push_str(&format!(" [animator {}]", a.clip));
            }
            if e.light.is_some() {
                extras.push_str(" [light]");
            }
            if e.shadow.is_some() {
                extras.push_str(" [shadow]");
            }
            out.push_str(&format!(
                "{:indent$}{} \"{}\"{mesh}{extras} at {:?}\n",
                "",
                e.id,
                e.name,
                t.translation.to_array(),
                indent = depth * 2 + 2
            ));
            tree(scene, db, Some(e.id), depth + 1, out);
        }
    }
    tree(&scene, &db, None, 0, &mut text);
    let assets: Vec<Value> = db
        .entries
        .iter()
        .map(|a| json!({"id": a.id, "path": a.relative, "kind": a.kind}))
        .collect();
    Ok(Success {
        payload: json!({
            "path": project.relative(&path),
            "scene": scene,
            "assets": assets,
        }),
        text,
    })
}

fn apply(
    root: &Path,
    scene: Option<&Path>,
    batch_path: &Path,
    dry_run: bool,
) -> Result<Success, Failure> {
    let project = Project::open(root)?;
    if !dry_run {
        project.check_unlocked()?;
    }
    let path = scene_path(&project, scene);
    let text = if batch_path == Path::new("-") {
        std::io::read_to_string(std::io::stdin())
            .map_err(|e| Failure::new(4, format!("reading stdin: {e}")))?
    } else {
        std::fs::read_to_string(batch_path)
            .map_err(|e| Failure::new(4, format!("{}: {e}", batch_path.display())))?
    };
    let batch = parse_batch(&text).map_err(|e| Failure::new(1, e.to_string()))?;
    let scene = load_scene(&path).map_err(|e| Failure::new(4, e.to_string()))?;
    let (edited, report) = apply_batch(&scene, &batch).map_err(|e| {
        let code = if matches!(e, BatchError::RevisionMismatch { .. }) {
            3
        } else {
            1
        };
        let mut f = Failure::new(code, e.to_string());
        if let BatchError::Command { index, op, .. } = &e {
            f.extra = json!({"failed_command": index, "failed_op": op});
        }
        f
    })?;
    // Asset references must resolve in this project.
    let db = AssetDatabase::scan(&project);
    let refs = copperframe_assets::project::check_scene_refs(&edited, &db, &[], None);
    if let Some(d) = refs
        .iter()
        .find(|d| d.severity == copperframe_assets::Severity::Error)
    {
        return Err(Failure::new(1, format!("batch rejected: {}", d.message)));
    }
    if !dry_run {
        // Re-check right before writing so a concurrent writer is not overwritten.
        project.check_unlocked()?;
        let current = load_scene(&path).map_err(|e| Failure::new(4, e.to_string()))?;
        if current.revision != scene.revision {
            return Err(Failure::new(
                3,
                format!(
                    "{} changed while the batch was applied (revision {} -> {}); nothing was written",
                    path.display(),
                    scene.revision,
                    current.revision
                ),
            ));
        }
        save_scene(&edited, &path).map_err(|e| Failure::new(4, e.to_string()))?;
    }
    let mut out = format!(
        "{} {} commands to {} (revision {} -> {})\n",
        if dry_run { "validated" } else { "applied" },
        report.commands.len(),
        project.relative(&path),
        report.previous_revision,
        report.revision
    );
    for c in &report.commands {
        for id in &c.created {
            out.push_str(&format!("  [{}] {} created {id}\n", c.index, c.op));
        }
    }
    Ok(Success {
        payload: json!({
            "scene": project.relative(&path),
            "dry_run": dry_run,
            "report": report,
        }),
        text: out,
    })
}
