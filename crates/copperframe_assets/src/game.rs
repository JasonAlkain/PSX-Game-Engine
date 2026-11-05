//! Finding and running a project's game executable, shared by the editor
//! (Play), `copperframe test-scene` and `copperframe build`.
//!
//! A project with gameplay code has a Rust crate in `game/` (created by
//! `copperframe new`); it is built and run with `cargo`. A project without one runs
//! in `copperframe_player`, the stock player with only the engine's `psx.*`
//! behaviours, found next to the running tool's executable.
//!
//! Every game executable accepts the arguments of `copperframe_game::launch`
//! (`--project`, `--scene`, `--headless`, `--script`, `--schema`, `--play`).

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::project::Project;

/// Folder of a project's game crate.
pub const GAME_DIR: &str = "game";
/// Behaviour schema cached by the editor.
pub const SCHEMA_CACHE: &str = ".psx/game_schema.json";
/// The stock player's executable name (without extension).
pub const STOCK_PLAYER: &str = "copperframe_player";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameTarget {
    /// `game/Cargo.toml`, built with cargo.
    Crate { manifest: PathBuf },
    /// A prebuilt executable (the stock player, or an exported game).
    Executable { path: PathBuf },
}

impl GameTarget {
    /// The project's game crate if it has one, else the stock player.
    pub fn for_project(project: &Project) -> Result<GameTarget, String> {
        let manifest = project.root.join(GAME_DIR).join("Cargo.toml");
        if manifest.is_file() {
            repair_manifest_paths(&manifest);
            let manifest = std::path::absolute(&manifest).map_err(|e| e.to_string())?;
            return Ok(GameTarget::Crate { manifest });
        }
        stock_player().map(|path| GameTarget::Executable { path })
    }

    pub fn describe(&self) -> String {
        match self {
            GameTarget::Crate { manifest } => format!("game crate {}", manifest.display()),
            GameTarget::Executable { path } => format!("executable {}", path.display()),
        }
    }

    /// A command that runs the game with `args`. For a crate this is
    /// `cargo run`, which builds first (its build output goes to stderr).
    /// It runs in the crate's folder so a `rust-toolchain.toml` there
    /// applies; paths in `args` must therefore be absolute.
    pub fn command(&self, args: &[OsString]) -> Command {
        match self {
            GameTarget::Crate { manifest } => {
                let mut c = Command::new(cargo());
                if let Some(dir) = manifest.parent() {
                    c.current_dir(dir);
                }
                c.arg("run")
                    .arg("--quiet")
                    .arg("--manifest-path")
                    .arg(manifest)
                    .arg("--")
                    .args(args);
                c
            }
            GameTarget::Executable { path } => {
                let mut c = Command::new(path);
                c.args(args);
                c
            }
        }
    }

    /// Builds a release executable and returns its path. Build output is
    /// passed to `log` line by line.
    pub fn build_release(&self, log: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
        let manifest = match self {
            GameTarget::Executable { path } => return Ok(path.clone()),
            GameTarget::Crate { manifest } => manifest,
        };
        let mut child = Command::new(cargo())
            .current_dir(manifest.parent().unwrap_or(Path::new(".")))
            .arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(manifest)
            .arg("--message-format=json-render-diagnostics")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run cargo ({e}); is Rust installed?"))?;
        // Diagnostics and progress arrive on stderr; drain it on a thread so
        // neither pipe can fill up and block cargo.
        let stderr = child.stderr.take();
        let err_thread = std::thread::spawn(move || {
            let mut lines = Vec::new();
            if let Some(s) = stderr {
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    lines.push(line);
                }
            }
            lines
        });
        let mut executable = None;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if msg["reason"] == "compiler-artifact"
                    && msg["target"]["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|k| k == "bin"))
                    && let Some(exe) = msg["executable"].as_str()
                {
                    executable = Some(PathBuf::from(exe));
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        for line in err_thread.join().unwrap_or_default() {
            log(&line);
        }
        if !status.success() {
            return Err(format!("cargo build failed ({status})"));
        }
        executable.ok_or_else(|| "cargo did not report a game executable".to_owned())
    }

    /// Runs the game with `--schema` for the project at `project_root`
    /// (its scripts are behaviour kinds too) and returns the behaviour
    /// schema.
    pub fn schema(&self, project_root: &Path) -> Result<serde_json::Value, String> {
        let root = std::path::absolute(project_root).map_err(|e| e.to_string())?;
        let out = self
            .command(&["--schema".into(), "--project".into(), root.into()])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("could not start the game: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(12).collect();
            return Err(format!(
                "the game failed to report its schema ({}):\n{}",
                out.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ));
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("the game printed an invalid schema: {e}"))?;
        if v["format"] != "psx_game_schema" {
            return Err("the game printed something that is not a psx_game_schema".into());
        }
        Ok(v)
    }
}

/// A headless run of a project's game: `--headless` (a test) or `--bench`.
/// Paths must be absolute: the game runs in its crate's folder.
#[derive(Debug, Clone, Default)]
pub struct HeadlessRun {
    pub project_root: PathBuf,
    /// Scene to play (default: the project's startup scene).
    pub scene: Option<PathBuf>,
    /// Test script driving the run.
    pub script: Option<PathBuf>,
    /// Ticks to run idle when there is no script.
    pub ticks: u64,
}

impl HeadlessRun {
    fn base_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--project".into(), self.project_root.clone().into()];
        if let Some(scene) = &self.scene {
            args.extend(["--scene".into(), scene.clone().into()]);
        }
        args
    }

    /// Plays the scene headless (no window, GPU or audio) and returns the
    /// game's run report (`passed`, `failures`, `events`, ...).
    pub fn test(&self, target: &GameTarget) -> Result<serde_json::Value, String> {
        let mut args = self.base_args();
        args.push("--headless".into());
        match &self.script {
            Some(s) => args.extend(["--script".into(), s.clone().into()]),
            None => args.extend(["--ticks".into(), self.ticks.to_string().into()]),
        }
        report_of(target.command(&args), &target.describe())
    }

    /// Measures frame and tick times offscreen with the release executable
    /// `exe` (see [`GameTarget::build_release`]).
    pub fn bench(&self, exe: &Path, frames: u32, size: &str) -> Result<serde_json::Value, String> {
        let mut args = self.base_args();
        args.extend([
            "--bench".into(),
            "--frames".into(),
            frames.to_string().into(),
            "--bench-size".into(),
            size.into(),
        ]);
        if let Some(s) = &self.script {
            args.extend(["--script".into(), s.clone().into()]);
        }
        let mut cmd = Command::new(exe);
        cmd.args(&args);
        report_of(cmd, &exe.display().to_string())
    }
}

/// Runs `cmd` and parses the JSON report it prints on stdout.
fn report_of(mut cmd: Command, what: &str) -> Result<serde_json::Value, String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {what}: {e}"))?;
    serde_json::from_slice(&out.stdout).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(20).collect();
        format!(
            "{what} did not produce a report ({}):\n{}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
    })
}

/// Text summary of a test run report, and whether it passed.
pub fn test_summary(report: &serde_json::Value) -> (bool, String) {
    // A scene that could not be played has no ticks.
    let mut text = if report["ticks"].is_u64() {
        format!(
            "{} ran {} ticks ({:.1} s) of {}\n",
            report["game"].as_str().unwrap_or("game"),
            report["ticks"],
            report["seconds"].as_f64().unwrap_or(0.0),
            report["scene"].as_str().unwrap_or("?")
        )
    } else {
        String::new()
    };
    for e in report["events"].as_array().into_iter().flatten() {
        text.push_str(&format!("  {e}\n"));
    }
    for w in report["warnings"].as_array().into_iter().flatten() {
        text.push_str(&format!("  warning: {}\n", w.as_str().unwrap_or_default()));
    }
    if let Some(o) = report["outcome"].as_object() {
        text.push_str(&format!(
            "outcome: {}\n",
            serde_json::Value::Object(o.clone())
        ));
    }
    let passed = report["passed"] == true;
    if passed {
        text.push_str("passed\n");
    } else {
        let failures: Vec<&str> = report["failures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
            .collect();
        text.push_str(&format!("failed:\n  {}\n", failures.join("\n  ")));
    }
    (passed, text)
}

/// Text summary of a benchmark report.
pub fn bench_summary(report: &serde_json::Value) -> String {
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
    text
}

/// Whether `cargo` runs (Rust is installed). Projects with a game crate
/// need it to Play, test and export.
pub fn rust_available() -> bool {
    Command::new(cargo())
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Where to get Rust.
pub const RUST_INSTALL_URL: &str = "https://rustup.rs";

/// `cargo`, or `$CARGO` when running under cargo.
pub fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

pub fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// The stock player next to the current executable.
pub fn stock_player() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the running executable has no folder")?;
    let p = dir.join(exe_name(STOCK_PLAYER));
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!(
            "this project has no game crate ({GAME_DIR}/Cargo.toml) and the stock player was not \
             found at {}; build it with `cargo build -p copperframe_player`",
            p.display()
        ))
    }
}

/// Reads the schema the editor cached, if present and valid.
pub fn cached_schema(project: &Project) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(project.root.join(SCHEMA_CACHE)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    (v["format"] == "psx_game_schema").then_some(v)
}

pub fn write_schema_cache(project: &Project, schema: &serde_json::Value) -> Result<(), String> {
    let path = project.root.join(SCHEMA_CACHE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(schema).unwrap_or_default() + "\n",
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether `dir` looks like the engine source tree (for `copperframe new`).
pub fn is_engine_root(dir: &Path) -> bool {
    dir.join("crates/copperframe_game/Cargo.toml").is_file()
}

/// Removes the Windows verbatim prefix from the paths in a game crate's
/// manifest. Earlier versions wrote `//?/D:/...` on Windows, which cargo
/// refuses to parse, so projects created by them could not build. Failures
/// are left for cargo to report.
fn repair_manifest_paths(manifest: &Path) {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return;
    };
    if let Some(fixed) = without_verbatim_paths(&text) {
        let _ = copperframe_core::fsutil::atomic_write(manifest, fixed.as_bytes());
    }
}

/// The manifest text with verbatim path prefixes removed, if it has any.
fn without_verbatim_paths(text: &str) -> Option<String> {
    text.contains("'//?/")
        .then(|| text.replace("'//?/UNC/", "'//").replace("'//?/", "'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_manifest_paths_are_repaired() {
        let broken = "a = { path = '//?/D:/Game Engines/x/crates/a' }\n\
                      b = { path = '//?/UNC/server/share/crates/b' }\n";
        assert_eq!(
            without_verbatim_paths(broken).unwrap(),
            "a = { path = 'D:/Game Engines/x/crates/a' }\n\
             b = { path = '//server/share/crates/b' }\n"
        );
        assert_eq!(without_verbatim_paths("a = { path = 'D:/x' }\n"), None);
    }

    #[test]
    fn finding_the_game_crate_repairs_its_manifest() {
        let dir = std::env::temp_dir().join(format!("cf_repair_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = Project::create(&dir, "Repair").unwrap();
        let manifest = dir.join(GAME_DIR).join("Cargo.toml");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "a = { path = '//?/D:/x' }\n").unwrap();
        GameTarget::for_project(&project).unwrap();
        assert_eq!(
            std::fs::read_to_string(&manifest).unwrap(),
            "a = { path = 'D:/x' }\n"
        );
        drop(project);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
