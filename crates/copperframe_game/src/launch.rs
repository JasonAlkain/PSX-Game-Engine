//! Command line of every game executable. A game's `main` is:
//!
//! ```ignore
//! fn main() -> std::process::ExitCode {
//!     copperframe_game::main(|| Box::new(MyGame), env!("CARGO_MANIFEST_DIR"))
//! }
//! ```
//!
//! Exit codes: 0 success; 1 the scene cannot be played, a headless
//! expectation failed, or the benchmark could not run (no adapter); 2 usage
//! error; 4 project or file error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use copperframe_assets::project::{PROJECT_FILE, Project};

use crate::api::Game;
use crate::audio::Audio;
use crate::headless::{self, Script};
use crate::player::{self, PlayerOptions};
use crate::settings::UserDirs;
use crate::sim::Simulation;

/// Folder next to an exported executable that holds the project data.
pub const EXPORT_DATA_DIR: &str = "data";

#[derive(Parser, Debug)]
#[command(about = "Game built with Copperframe Engine")]
pub struct Args {
    /// Project folder (default: `data` next to the executable, else the
    /// development project the game was built from).
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// Scene to start, relative to the project (default: the startup scene).
    #[arg(long)]
    pub scene: Option<PathBuf>,
    /// Print the behaviour schema (JSON) and exit.
    #[arg(long)]
    pub schema: bool,
    /// Run without a window, GPU or audio and print a JSON report.
    #[arg(long)]
    pub headless: bool,
    /// Input script for a headless run (`psx_script` JSON). Implies --headless.
    #[arg(long)]
    pub script: Option<PathBuf>,
    /// Ticks to run headless without a script.
    #[arg(long, default_value_t = 600)]
    pub ticks: u64,
    /// Also write the headless report to this file.
    #[arg(long)]
    pub report: Option<PathBuf>,
    /// Editor play mode: skip the title screen and never write saves.
    #[arg(long)]
    pub play: bool,
    /// Exit after presenting this many frames (smoke tests). With
    /// `--bench`: measured frames per style (default 300).
    #[arg(long)]
    pub frames: Option<u64>,
    /// Measure tick and frame times offscreen (no window or audio) in the
    /// authored, PS1 and PS2 styles and print a JSON report. Uses
    /// `--script` as input when given.
    #[arg(long)]
    pub bench: bool,
    /// Output size for `--bench`, `WIDTHxHEIGHT`.
    #[arg(long, default_value = "1280x720", value_parser = parse_size)]
    pub bench_size: (u32, u32),
    /// Folder for settings and saves (default: the user's per-game folders).
    #[arg(long)]
    pub user_dir: Option<PathBuf>,
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, found `{s}`"))?;
    let parse = |v: &str| {
        v.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (16..=8192).contains(n))
            .ok_or_else(|| format!("`{v}` is not a size in 16..=8192"))
    };
    Ok((parse(w)?, parse(h)?))
}

/// Parses the process arguments and runs the game.
pub fn main(create: fn() -> Box<dyn Game>, dev_project: &str) -> ExitCode {
    install_crash_reports(&create().title());
    copperframe_runtime::init_logging();
    let args = match Args::try_parse() {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    run(args, create, Path::new(dev_project))
}

/// Crash reports and the log file of a game go to its own data folder
/// (next to its settings and saves) and never offer the engine's report
/// address: players of a game report to its author, not to the engine.
/// The editor's Play redirects them with `COPPERFRAME_CRASH_DIR`.
fn install_crash_reports(title: &str) {
    let app = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "game".to_string());
    let dir = crate::settings::UserDirs::for_game(title).map(|d| d.data);
    copperframe_core::report::install(&app, dir, None);
}

/// The project a game opens without `--project`.
pub fn default_project(dev_project: &Path) -> PathBuf {
    let exported = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(EXPORT_DATA_DIR)));
    match exported {
        Some(d) if d.join(PROJECT_FILE).is_file() => d,
        _ => dev_project.to_path_buf(),
    }
}

pub fn run(args: Args, create: fn() -> Box<dyn Game>, dev_project: &Path) -> ExitCode {
    if args.schema {
        let game = create();
        let mut registry = crate::api::Registry::with_builtins();
        game.register(&mut registry);
        let schema = registry.schema(&game.title());
        println!(
            "{}",
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }
    let root = args
        .project
        .clone()
        .unwrap_or_else(|| default_project(dev_project));
    let project = match Project::open(&root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(4);
        }
    };
    let scene = args
        .scene
        .clone()
        .unwrap_or_else(|| PathBuf::from(&project.file.startup_scene));

    if args.bench {
        return run_bench(&args, project, &scene, create);
    }
    if args.headless || args.script.is_some() {
        return run_headless(&args, project, &scene, create);
    }
    let options = PlayerOptions {
        project,
        scene,
        skip_title: args.play,
        exit_after_frames: args.frames,
        dirs: args.user_dir.as_deref().map(UserDirs::at),
    };
    match player::run(options, create()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn read_script(path: &Path) -> Result<Script, ExitCode> {
    match std::fs::read_to_string(path) {
        Ok(text) => Script::parse(&text).map_err(|e| {
            eprintln!("error: {}: {e}", path.display());
            ExitCode::from(2)
        }),
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            Err(ExitCode::from(4))
        }
    }
}

fn run_bench(
    args: &Args,
    project: Project,
    scene: &Path,
    create: fn() -> Box<dyn Game>,
) -> ExitCode {
    let script = match &args.script {
        Some(path) => match read_script(path) {
            Ok(s) => Some(s),
            Err(code) => return code,
        },
        None => None,
    };
    let options = crate::bench::BenchOptions {
        frames: args
            .frames
            .unwrap_or(300)
            .min(u64::from(crate::bench::MAX_BENCH_FRAMES)) as u32,
        size: args.bench_size,
        script,
    };
    match crate::bench::run(project, scene, create, &options) {
        Ok(report) => {
            let text = serde_json::to_string_pretty(&report).unwrap_or_default();
            println!("{text}");
            if let Some(path) = &args.report
                && let Err(e) = std::fs::write(path, format!("{text}\n"))
            {
                eprintln!("error: could not write {}: {e}", path.display());
                return ExitCode::from(4);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_headless(
    args: &Args,
    project: Project,
    scene: &Path,
    create: fn() -> Box<dyn Game>,
) -> ExitCode {
    let script = match &args.script {
        Some(path) => match read_script(path) {
            Ok(s) => s,
            Err(code) => return code,
        },
        None => Script::idle(args.ticks.min(headless::MAX_TICKS)),
    };
    let mut sim = Simulation::new(project, create(), Audio::silent());
    let report = match sim.load_scene_file(scene) {
        Ok(()) => headless::run(&mut sim, &script),
        Err(e) => serde_json::json!({
            "format": headless::REPORT_FORMAT,
            "version": 1,
            "game": sim.title(),
            "passed": false,
            "failures": [e.to_string()],
        }),
    };
    sim.shutdown();
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    println!("{text}");
    if let Some(path) = &args.report
        && let Err(e) = std::fs::write(path, format!("{text}\n"))
    {
        eprintln!("error: could not write {}: {e}", path.display());
        return ExitCode::from(4);
    }
    if report["passed"] == serde_json::Value::Bool(true) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
