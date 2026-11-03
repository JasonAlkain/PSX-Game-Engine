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

    /// Runs the game with `--schema` and returns the behaviour schema.
    pub fn schema(&self) -> Result<serde_json::Value, String> {
        let out = self
            .command(&["--schema".into()])
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
