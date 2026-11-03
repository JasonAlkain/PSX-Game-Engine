//! `psx`: project tools without the GUI.
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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use psx_assets::project::{AssetDatabase, ImportedAsset, Project, ProjectError};
use psx_assets::validate_project;
use psx_scene::batch::{BATCH_FORMAT, BATCH_VERSION, BatchError, parse_batch};
use psx_scene::file::{SCENE_FORMAT, SCENE_VERSION};
use psx_scene::{Command, apply_batch, load_scene, save_scene};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "psx", version, about = "PSX Engine project tools")]
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
    },
    /// Describe file formats, components and batch commands.
    Schema,
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 2 } else { 0 });
        }
    };
    let name = match &cli.command {
        Cmd::New { .. } => "new",
        Cmd::Schema => "schema",
        Cmd::Validate { .. } => "validate",
        Cmd::Import { .. } => "import",
        Cmd::Inspect { .. } => "inspect",
        Cmd::Apply { .. } => "apply",
    };
    let result = match cli.command {
        Cmd::New { dir, name } => new(&dir, name),
        Cmd::Schema => Ok(schema()),
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

fn new(dir: &Path, name: Option<String>) -> Result<Success, Failure> {
    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    });
    let project = Project::create(dir, &name)?;
    let scene = project.startup_scene_path();
    Ok(Success {
        payload: json!({
            "project": dir.display().to_string(),
            "name": name,
            "startup_scene": project.file.startup_scene,
        }),
        text: format!(
            "created project \"{name}\" in {}\n  startup scene: {}\n  \
             (a Rust gameplay template arrives with play mode in Milestone 4)\n",
            dir.display(),
            scene.display()
        ),
    })
}

fn schema() -> Success {
    let ops: Vec<Value> = Command::OPS
        .iter()
        .map(|(op, args)| json!({"op": op, "fields": args}))
        .collect();
    let payload = json!({
        "formats": {
            "project": {"format": psx_assets::project::PROJECT_FORMAT, "version": psx_assets::project::PROJECT_VERSION},
            "scene": {"format": SCENE_FORMAT, "version": SCENE_VERSION},
            "command_batch": {"format": BATCH_FORMAT, "version": BATCH_VERSION},
            "asset_meta": {"format": psx_assets::meta::META_FORMAT, "version": psx_assets::meta::META_VERSION},
            "style": {"format": psx_core::style::STYLE_FORMAT, "version": psx_core::style::STYLE_VERSION},
        },
        "importer_version": psx_assets::IMPORTER_VERSION,
        "asset_kinds": {
            "gltf": [".gltf", ".glb"],
            "texture": [".png", ".jpg", ".jpeg"],
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
        },
        "conventions": {
            "coordinates": "right-handed, +Y up, meters, camera looks along -Z",
            "rotation_euler_deg": "[x, y, z] degrees; applied about Y, then X, then Z (intrinsic)",
            "ids": "UUID strings; entity ids may be chosen by the caller in create_entity",
        },
        "commands": ops,
    });
    let mut text = String::from("formats:\n");
    for (k, v) in payload["formats"].as_object().into_iter().flatten() {
        text.push_str(&format!("  {k}: {} v{}\n", v["format"], v["version"]));
    }
    text.push_str("batch commands:\n");
    for (op, args) in Command::OPS {
        text.push_str(&format!("  {op:<18} {args}\n"));
    }
    Success { payload, text }
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
                    }),
                    ImportedAsset::Texture(t) => json!({"width": t.width, "height": t.height}),
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
        scene: &psx_scene::Scene,
        db: &AssetDatabase,
        parent: Option<psx_core::EntityId>,
        depth: usize,
        out: &mut String,
    ) {
        for e in scene.children(parent) {
            let t = &e.transform;
            let mesh = match e.mesh.as_ref().map(|m| &m.source) {
                Some(psx_scene::MeshSource::Primitive { shape, .. }) => {
                    format!(" [{}]", shape.label())
                }
                Some(psx_scene::MeshSource::Model { asset }) => format!(
                    " [model {}]",
                    db.get(*asset)
                        .map_or("MISSING".to_owned(), |a| a.relative.clone())
                ),
                None => String::new(),
            };
            out.push_str(&format!(
                "{:indent$}{} \"{}\"{mesh} at {:?}\n",
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
    let refs = psx_assets::project::check_scene_refs(&edited, &db, &[], None);
    if let Some(d) = refs
        .iter()
        .find(|d| d.severity == psx_assets::Severity::Error)
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
