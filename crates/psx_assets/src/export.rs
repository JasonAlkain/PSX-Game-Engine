//! Host-platform export: a folder with the release game executable, the
//! project data it loads (`data/`), third-party notices, and a short
//! README. The executable finds `data/` next to itself, so the folder can be
//! moved anywhere.
//!
//! ```text
//! <out>/
//!   <game>[.exe]
//!   data/project.json, data/input.json, data/scenes/…, data/assets/…
//!   NOTICES.txt      third-party crates and licences
//!   README.txt
//!   export.json      what was exported, by which engine version
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::game::{GameTarget, cargo};
use crate::project::{ASSETS_DIR, PROJECT_FILE, Project, SCENES_DIR};
use crate::validate_project;

/// Marker written into every export folder; only folders with it are
/// replaced by a later export.
pub const EXPORT_MARKER: &str = "export.json";
pub const DATA_DIR: &str = "data";

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub out: PathBuf,
    /// Replace an earlier export in `out`.
    pub replace: bool,
    /// Cargo manifest whose dependency graph is listed in NOTICES.txt.
    pub notices_manifest: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ExportReport {
    pub out: PathBuf,
    pub executable: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub crates: usize,
}

/// Validates, builds and writes the export. `log` receives build output.
pub fn export(
    project: &Project,
    target: &GameTarget,
    options: &ExportOptions,
    log: &mut dyn FnMut(&str),
) -> Result<ExportReport, String> {
    let report = validate_project(&project.root);
    if !report.ok() {
        return Err(format!(
            "the project has validation errors; fix them first:\n{}",
            report
                .diagnostics
                .iter()
                .filter(|d| d.severity == crate::project::Severity::Error)
                .map(|e| format!("  {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let out = absolute(&options.out)?;
    let root = absolute(&project.root)?;
    for data in [ASSETS_DIR, SCENES_DIR] {
        if out.starts_with(root.join(data)) {
            return Err(format!(
                "the export folder cannot be inside the project's {data}/ folder"
            ));
        }
    }
    prepare_out(&out, options.replace)?;

    log("building the release executable…");
    let built = target.build_release(log)?;
    let exe_name = match target {
        GameTarget::Crate { .. } => built
            .file_name()
            .map(PathBuf::from)
            .ok_or("the game executable has no file name")?,
        // The stock player is renamed after the project.
        GameTarget::Executable { .. } => {
            PathBuf::from(crate::game::exe_name(&file_slug(&project.file.name)))
        }
    };
    let mut files = 0usize;
    let mut bytes = 0u64;
    let exe = out.join(&exe_name);
    bytes += copy(&built, &exe)?;
    files += 1;

    let data = out.join(DATA_DIR);
    for (rel, src) in data_files(&root)? {
        bytes += copy(&src, &data.join(&rel))?;
        files += 1;
    }

    log("listing third-party licences…");
    let crates = dependency_notices(&options.notices_manifest)?;
    let mut notices = String::from(
        "Third-party software in this game\n\
         =================================\n\n\
         Built with PSX Engine (MIT licence). The executable links the following\n\
         Rust crates (the list is the full dependency graph, so it may include\n\
         platform-specific crates not used on this platform):\n\n",
    );
    for c in &crates {
        notices.push_str(&format!(
            "{} {} — {}{}\n",
            c.name,
            c.version,
            c.license,
            c.repository
                .as_deref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default()
        ));
    }
    write(&out.join("NOTICES.txt"), &notices)?;
    write(
        &out.join("README.txt"),
        &format!(
            "{name}\n\nRun {exe}. Settings and saves are stored in your user folder, not here.\n\
             `{exe} --help` lists command-line options (for example --headless).\n\n\
             Made with PSX Engine (PS1- and PS2-style 3D).\n",
            name = project.file.name,
            exe = exe_name.display()
        ),
    )?;
    let marker = json!({
        "format": "psx_export",
        "version": 1,
        "game": project.file.name,
        "executable": exe_name,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "files": files,
        "bytes": bytes,
        "crates": crates.len(),
    });
    write(
        &out.join(EXPORT_MARKER),
        &(serde_json::to_string_pretty(&marker).unwrap_or_default() + "\n"),
    )?;
    Ok(ExportReport {
        out,
        executable: exe,
        files: files + 3,
        bytes,
        crates: crates.len(),
    })
}

/// Runs an exported executable headless from a temporary working folder
/// whose name contains spaces, so relative-path assumptions show up.
/// Returns the game's report.
pub fn verify_export(exe: &Path, script: Option<&Path>, ticks: u64) -> Result<Value, String> {
    let cwd = std::env::temp_dir().join(format!("psx export check {}", std::process::id()));
    std::fs::create_dir_all(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut c = Command::new(exe);
    c.current_dir(&cwd).arg("--headless");
    match script {
        Some(s) => {
            c.arg("--script").arg(absolute(s)?);
        }
        None => {
            c.arg("--ticks").arg(ticks.to_string());
        }
    }
    let out = c
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not start {}: {e}", exe.display()));
    let _ = std::fs::remove_dir_all(&cwd);
    let out = out?;
    let report: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "the exported game did not print a report ({e}); stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })?;
    if !out.status.success() || report["passed"] != true {
        return Err(format!(
            "the exported game's headless run failed ({}): {}",
            out.status, report["failures"]
        ));
    }
    Ok(report)
}

fn absolute(p: &Path) -> Result<PathBuf, String> {
    std::path::absolute(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Checks, without changing anything, that `out` may receive an export:
/// it is missing, empty, or an earlier export and `replace` is set. Callers
/// can run this before a slow build to fail early.
pub fn check_output_folder(out: &Path, replace: bool) -> Result<(), String> {
    if !out.exists() {
        return Ok(());
    }
    let empty = std::fs::read_dir(out)
        .map_err(|e| format!("{}: {e}", out.display()))?
        .next()
        .is_none();
    if empty {
        return Ok(());
    }
    if !out.join(EXPORT_MARKER).is_file() {
        return Err(format!(
            "{} is not empty and is not an earlier export; choose another folder",
            out.display()
        ));
    }
    if !replace {
        return Err(format!(
            "{} holds an earlier export; pass --replace to overwrite it",
            out.display()
        ));
    }
    Ok(())
}

fn prepare_out(out: &Path, replace: bool) -> Result<(), String> {
    check_output_folder(out, replace)?;
    if out.join(EXPORT_MARKER).is_file() {
        std::fs::remove_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))
}

/// Project files the game loads at run time: the project file, input
/// bindings, scenes and assets (with their metadata). Editor state,
/// backups, scripts and the game's source are left out.
pub fn data_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut files = vec![(PathBuf::from(PROJECT_FILE), root.join(PROJECT_FILE))];
    let input = root.join("input.json");
    if input.is_file() {
        files.push(("input.json".into(), input));
    }
    for dir in [SCENES_DIR, ASSETS_DIR] {
        walk(&root.join(dir), &mut |path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let skip = name.starts_with('.') || name.ends_with(".bak") || name.ends_with(".tmp");
            let keep = !skip && (dir != SCENES_DIR || name.ends_with(".scene.json"));
            if keep && let Ok(rel) = path.strip_prefix(root) {
                files.push((rel.to_path_buf(), path.to_path_buf()));
            }
        })?;
    }
    files.sort();
    Ok(files)
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            walk(&path, f)?;
        } else if ty.is_file() {
            f(&path);
        }
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<u64, String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::copy(from, to).map_err(|e| format!("copying {}: {e}", from.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn file_slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_owned();
    if s.is_empty() { "game".into() } else { s }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CrateNotice {
    pub name: String,
    pub version: String,
    pub license: String,
    pub repository: Option<String>,
}

/// Crates reachable through normal and build dependencies from the package
/// at `manifest`, from `cargo metadata`.
pub fn dependency_notices(manifest: &Path) -> Result<Vec<CrateNotice>, String> {
    let out = Command::new(cargo())
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let manifest = std::fs::canonicalize(manifest).map_err(|e| e.to_string())?;
    let packages = meta["packages"]
        .as_array()
        .ok_or("cargo metadata: no packages")?;
    let root_id = packages
        .iter()
        .find(|p| {
            p["manifest_path"]
                .as_str()
                .and_then(|m| std::fs::canonicalize(m).ok())
                .is_some_and(|m| m == manifest)
        })
        .and_then(|p| p["id"].as_str())
        .ok_or("cargo metadata did not list the game package")?;
    let nodes: BTreeMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .ok_or("cargo metadata: no resolve graph")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![root_id];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let runtime = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null() || k["kind"] == "build");
            if runtime && let Some(pkg) = dep["pkg"].as_str() {
                stack.push(pkg);
            }
        }
    }
    let mut crates: Vec<CrateNotice> = packages
        .iter()
        .filter(|p| p["id"].as_str().is_some_and(|id| seen.contains(id)))
        .map(|p| CrateNotice {
            name: p["name"].as_str().unwrap_or("?").to_owned(),
            version: p["version"].as_str().unwrap_or("?").to_owned(),
            license: p["license"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    p["license_file"]
                        .as_str()
                        .map(|f| format!("see licence file {f}"))
                })
                .unwrap_or_else(|| "licence not declared".into()),
            repository: p["repository"].as_str().map(str::to_owned),
        })
        .collect();
    crates.sort();
    Ok(crates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_files_skip_editor_state_backups_and_sources() {
        let root = std::env::temp_dir().join(format!("psx_export_files_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Project::create(&root, "T").unwrap();
        for f in [
            "assets/a.png",
            "assets/a.png.meta.json",
            "assets/b.png.bak",
            "scenes/main.scene.json.bak",
            "scenes/notes.txt",
            ".psx/game_schema.json",
            "game/src/main.rs",
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        }
        let files: Vec<String> = data_files(&root)
            .unwrap()
            .into_iter()
            .map(|(r, _)| r.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            files,
            [
                "assets/a.png",
                "assets/a.png.meta.json",
                "project.json",
                "scenes/main.scene.json"
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_to_overwrite_foreign_folders() {
        let out = std::env::temp_dir().join(format!("psx_export_out_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("keep.txt"), "mine").unwrap();
        assert!(
            prepare_out(&out, true)
                .unwrap_err()
                .contains("not an earlier export")
        );
        std::fs::write(out.join(EXPORT_MARKER), "{}").unwrap();
        assert!(prepare_out(&out, false).unwrap_err().contains("--replace"));
        prepare_out(&out, true).unwrap();
        assert!(!out.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&out);
    }
}
