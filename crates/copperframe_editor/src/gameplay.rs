//! Gameplay authoring in the editor: inspectors for colliders, audio
//! sources and behaviours, Play/Stop (the game runs as a child process on a
//! temporary copy of the scene), the behaviour schema, and the Build menu's
//! background jobs (validate, export, test scripts, benchmark).
//!
//! Play never touches the authored scene: the current in-memory scene,
//! including unsaved edits, is written to `.psx/play/` and the game is
//! started with `--play --scene <that copy>`. Its output streams into the
//! console. Stopping kills the process.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};

use copperframe_assets::AssetKind;
use copperframe_assets::export::{ExportOptions, Notices, export, verify_export};
use copperframe_assets::game::{
    GameTarget, HeadlessRun, bench_summary, cached_schema, test_summary, write_schema_cache,
};
use copperframe_core::{AssetId, EntityId};
use copperframe_scene::{
    AudioBus, BodyKind, ColliderComponent, ColliderShape, Command, Entity, MeshSource,
    PrimitiveShape,
};
use egui::Color32;
use glam::Vec3;
use serde_json::Value;

use crate::session::{EditorSession, LogLevel};

/// Folder (inside the project's `.psx/`) for play-mode scene copies.
pub const PLAY_DIR: &str = ".psx/play";

enum Msg {
    Log(LogLevel, String),
    Schema(Result<Value, String>),
    JobDone(JobResult),
}

/// A Build menu job running in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    Validate,
    Export,
    Test,
    Bench,
}

impl Job {
    pub fn title(self) -> &'static str {
        match self {
            Job::Validate => "Validate project",
            Job::Export => "Export game",
            Job::Test => "Test script",
            Job::Bench => "Benchmark",
        }
    }

    fn busy(self) -> &'static str {
        match self {
            Job::Validate => "validating…",
            Job::Export => "exporting…",
            Job::Test => "running the test…",
            Job::Bench => "benchmarking…",
        }
    }
}

/// The outcome of a finished [`Job`], shown in the results window.
#[derive(Debug, Clone)]
pub struct JobResult {
    pub job: Job,
    pub passed: bool,
    /// One line for the console.
    pub headline: String,
    /// The full report.
    pub details: String,
    /// A folder to offer opening (the export).
    pub folder: Option<PathBuf>,
}

/// Child processes and background jobs started from the editor.
pub struct GameTools {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    play: Option<Child>,
    schema: Option<Value>,
    schema_busy: bool,
    job: Option<Job>,
    /// The last finished job, until the results window is closed.
    pub result: Option<JobResult>,
    /// `cargo` was checked once; `Some(false)` shows the install prompt.
    rust: Option<bool>,
    /// Show the "Rust is not installed" prompt.
    pub rust_prompt: bool,
    /// Set when a Play session exits with an error, so the app can look
    /// for its crash report.
    game_failed: bool,
    pub export_dir: String,
}

impl GameTools {
    pub fn new(session: &mut EditorSession) -> Self {
        let (tx, rx) = channel();
        let export_dir = session.project.export_dir().display().to_string();
        let mut tools = Self {
            tx,
            rx,
            play: None,
            schema: cached_schema(&session.project),
            schema_busy: false,
            job: None,
            result: None,
            rust: None,
            rust_prompt: false,
            game_failed: false,
            export_dir,
        };
        if tools.schema.is_none() && tools.can_build(session, false) {
            tools.refresh_schema(session);
        }
        tools
    }

    /// Whether the project's game can be built: always for the stock
    /// player; for a game crate only if Rust is installed. With `prompt`,
    /// a missing Rust shows the install prompt.
    pub fn can_build(&mut self, session: &mut EditorSession, prompt: bool) -> bool {
        if !matches!(
            GameTarget::for_project(&session.project),
            Ok(GameTarget::Crate { .. })
        ) {
            return true;
        }
        let ok = *self
            .rust
            .get_or_insert_with(copperframe_assets::game::rust_available);
        if !ok {
            if prompt {
                self.rust_prompt = true;
            }
            session.warn(
                "this project has Rust game code (game/), but Rust is not installed; \
                 install it from https://rustup.rs and restart the editor",
            );
        }
        ok
    }

    /// Checks for Rust again (after the user installed it).
    pub fn recheck_rust(&mut self, session: &mut EditorSession) -> bool {
        self.rust = None;
        let ok = self.can_build(session, false);
        if ok {
            self.rust_prompt = false;
            session.info("Rust found");
        }
        ok
    }

    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    pub fn busy_label(&self) -> Option<&'static str> {
        if let Some(job) = self.job {
            Some(job.busy())
        } else if self.schema_busy {
            Some("reading behaviours…")
        } else {
            None
        }
    }

    pub fn schema(&self) -> Option<&Value> {
        self.schema.as_ref()
    }

    /// Drains background output into the console; notices when the game
    /// exits by itself.
    pub fn poll(&mut self, session: &mut EditorSession) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Log(level, text) => session.push_log(level, text),
                Msg::Schema(result) => {
                    self.schema_busy = false;
                    match result {
                        Ok(schema) => {
                            if let Err(e) = write_schema_cache(&session.project, &schema) {
                                session.warn(e);
                            }
                            let n = schema["behaviors"].as_array().map_or(0, Vec::len);
                            session.info(format!("behaviour schema loaded ({n} kinds)"));
                            self.schema = Some(schema);
                        }
                        Err(e) => session.error(e),
                    }
                }
                Msg::JobDone(result) => {
                    self.job = None;
                    let level = if result.passed {
                        LogLevel::Info
                    } else {
                        LogLevel::Error
                    };
                    session.push_log(level, &result.headline);
                    self.result = Some(result);
                }
            }
        }
        if let Some(child) = &mut self.play {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let level = if status.success() {
                        LogLevel::Info
                    } else {
                        LogLevel::Error
                    };
                    session.push_log(level, format!("game exited ({status})"));
                    self.game_failed |= !status.success();
                    self.play = None;
                }
                Ok(None) => {}
                Err(e) => {
                    session.error(format!("lost track of the game process: {e}"));
                    self.play = None;
                }
            }
        }
    }

    /// Starts the game on a copy of the current scene.
    pub fn play(&mut self, session: &mut EditorSession) {
        if self.play.is_some() || !self.can_build(session, true) {
            return;
        }
        let target = match GameTarget::for_project(&session.project) {
            Ok(t) => t,
            Err(e) => {
                session.error(e);
                return;
            }
        };
        let dir = session.project.root.join(PLAY_DIR);
        let name = session
            .scene_path
            .file_name()
            .map_or_else(|| "play.scene.json".into(), |n| n.to_os_string());
        let copy = dir.join(name);
        if let Err(e) = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                copperframe_scene::save_scene(&session.scene, &copy).map_err(|e| e.to_string())
            })
        {
            session.error(format!("could not write the play copy: {e}"));
            return;
        }
        let root = match std::path::absolute(&session.project.root) {
            Ok(r) => r,
            Err(e) => {
                session.error(e.to_string());
                return;
            }
        };
        let copy = root
            .join(PLAY_DIR)
            .join(copy.file_name().unwrap_or_default());
        let args: Vec<OsString> = vec![
            "--project".into(),
            root.into(),
            "--scene".into(),
            copy.into(),
            "--play".into(),
        ];
        let mut cmd = target.command(&args);
        // The game's crash reports go where the editor looks for its own.
        if let Some(dir) = copperframe_core::report::data_dir() {
            cmd.env(copperframe_core::report::CRASH_DIR_ENV, dir);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(mut child) => {
                session.info(format!(
                    "playing with {} (builds first if needed; the authored scene is not changed)",
                    target.describe()
                ));
                for (pipe, level) in [
                    (
                        child
                            .stdout
                            .take()
                            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                        LogLevel::Info,
                    ),
                    (
                        child
                            .stderr
                            .take()
                            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                        LogLevel::Info,
                    ),
                ] {
                    if let Some(pipe) = pipe {
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                                let level = classify(&line).unwrap_or(level);
                                if tx
                                    .send(Msg::Log(level, format!("[game] {}", strip_ansi(&line))))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });
                    }
                }
                self.play = Some(child);
                if self.schema.is_none() {
                    self.refresh_schema(session);
                }
            }
            Err(e) => session.error(format!("could not start {}: {e}", target.describe())),
        }
    }

    /// True once after a Play session exited with an error.
    pub fn take_game_failed(&mut self) -> bool {
        std::mem::take(&mut self.game_failed)
    }

    pub fn stop(&mut self, session: &mut EditorSession) {
        if let Some(mut child) = self.play.take() {
            let _ = child.kill();
            let _ = child.wait();
            session.info("stopped the game");
        }
    }

    /// Asks the game for its behaviour kinds (in the background; a game
    /// crate may need to compile first).
    pub fn refresh_schema(&mut self, session: &EditorSession) {
        if self.schema_busy {
            return;
        }
        let target = match GameTarget::for_project(&session.project) {
            Ok(t) => t,
            Err(e) => {
                let _ = self.tx.send(Msg::Schema(Err(e)));
                return;
            }
        };
        self.schema_busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Schema(target.schema()));
        });
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// Runs `work` on a background thread as `job`; its log lines stream
    /// into the console.
    fn spawn_job(
        &mut self,
        job: Job,
        work: impl FnOnce(&mut dyn FnMut(&str)) -> JobResult + Send + 'static,
    ) {
        self.job = Some(job);
        self.result = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let log_tx = tx.clone();
            let mut log = |line: &str| {
                let _ = log_tx.send(Msg::Log(
                    LogLevel::Info,
                    format!("[{}] {}", job.title().to_lowercase(), strip_ansi(line)),
                ));
            };
            let result = work(&mut log);
            let _ = tx.send(Msg::JobDone(result));
        });
    }

    /// Checks the project (Build > Validate): the project file, every asset
    /// and every scene. Uses the files on disk; the caller saves first.
    pub fn validate(&mut self, session: &mut EditorSession) {
        if self.is_busy() {
            return;
        }
        let root = session.project.root.clone();
        session.info("validating the project…");
        self.spawn_job(Job::Validate, move |_| {
            let report = copperframe_assets::validate_project(&root);
            let mut details = String::new();
            for d in &report.diagnostics {
                details.push_str(&format!("{d}\n"));
            }
            let headline = format!(
                "validation: {} assets, {} scenes: {} errors, {} warnings",
                report.assets.len(),
                report.scenes.len(),
                report.errors,
                report.warnings
            );
            if details.is_empty() {
                details = "No problems found.\n".into();
            }
            JobResult {
                job: Job::Validate,
                passed: report.ok(),
                headline,
                details,
                folder: None,
            }
        });
    }

    /// Exports to [`GameTools::export_dir`] in the background, replacing an
    /// earlier export there, then runs the export headless as a check.
    /// Unsaved changes are not exported; the caller saves first.
    pub fn export(&mut self, session: &mut EditorSession) {
        if self.is_busy() || !self.can_build(session, true) {
            return;
        }
        let target = match GameTarget::for_project(&session.project) {
            Ok(t) => t,
            Err(e) => {
                session.error(e);
                return;
            }
        };
        let notices = match Notices::for_target(&target) {
            Ok(n) => n,
            Err(e) => {
                session.error(format!("export: {e}"));
                return;
            }
        };
        let project = session.project.clone();
        let out = PathBuf::from(self.export_dir.trim());
        let check = project
            .file
            .build
            .check_after_export
            .then(|| project.export_check_script());
        session.info(format!("exporting to {} …", out.display()));
        self.spawn_job(Job::Export, move |log| {
            let options = ExportOptions {
                out,
                replace: true,
                notices,
            };
            let result = export(&project, &target, &options, log).and_then(|r| match &check {
                Some(script) => verify_export(&r.executable, script.as_deref(), 120).map(|_| r),
                None => Ok(r),
            });
            let checked = if check.is_some() {
                " and checked that it starts"
            } else {
                ""
            };
            match result {
                Ok(r) => JobResult {
                    job: Job::Export,
                    passed: true,
                    headline: format!(
                        "exported to {} ({} files, {:.1} MiB){checked}",
                        r.out.display(),
                        r.files,
                        r.bytes as f64 / (1024.0 * 1024.0)
                    ),
                    details: format!(
                        "The game is in {}.\nRun {} to play it; zip the folder to share it.\n",
                        r.out.display(),
                        r.executable.display()
                    ),
                    folder: Some(r.out),
                },
                Err(e) => JobResult {
                    job: Job::Export,
                    passed: false,
                    headline: "export failed".into(),
                    details: e,
                    folder: None,
                },
            }
        });
    }

    /// Test scripts in the project (`*.script.json`, outside `assets/`).
    pub fn test_scripts(session: &EditorSession) -> Vec<PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>, depth: u32) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() {
                    if depth < 3
                        && !name.starts_with('.')
                        && !["target", "dist", "assets", "game"].contains(&name.as_str())
                    {
                        walk(&p, out, depth + 1);
                    }
                } else if name.ends_with(".script.json") {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(&session.project.root, &mut out, 0);
        out.sort();
        out
    }

    /// Plays the open scene headless, driven by `script` (or idle for a few
    /// seconds without one), and reports whether its expectations passed.
    /// Uses the scene on disk; the caller saves first.
    pub fn run_test(&mut self, session: &mut EditorSession, script: Option<PathBuf>) {
        if self.is_busy() || !self.can_build(session, true) {
            return;
        }
        let Some(target) = self.target(session) else {
            return;
        };
        let Some(run) = headless_run(session, script.clone()) else {
            return;
        };
        let label = script
            .as_deref()
            .map_or("an idle run".to_owned(), |s| session.project.relative(s));
        session.info(format!(
            "testing {label} (builds the game first if needed)…"
        ));
        self.spawn_job(Job::Test, move |_| match run.test(&target) {
            Ok(report) => {
                let (passed, details) = test_summary(&report);
                JobResult {
                    job: Job::Test,
                    passed,
                    headline: format!("test {label}: {}", if passed { "passed" } else { "FAILED" }),
                    details,
                    folder: None,
                }
            }
            Err(e) => JobResult {
                job: Job::Test,
                passed: false,
                headline: format!("test {label}: could not run"),
                details: e,
                folder: None,
            },
        });
    }

    /// Measures frame and tick times of the open scene with a release build
    /// (Build > Advanced > Benchmark).
    pub fn bench(&mut self, session: &mut EditorSession) {
        if self.is_busy() || !self.can_build(session, true) {
            return;
        }
        let Some(target) = self.target(session) else {
            return;
        };
        let Some(run) = headless_run(session, None) else {
            return;
        };
        session.info("benchmarking (release build first; this can take a few minutes)…");
        self.spawn_job(Job::Bench, move |log| {
            let result = target
                .build_release(log)
                .and_then(|exe| run.bench(&exe, 300, "1280x720"));
            match result {
                Ok(report) => {
                    let details = bench_summary(&report);
                    JobResult {
                        job: Job::Bench,
                        passed: true,
                        headline: format!(
                            "benchmark: {}",
                            details.lines().next().unwrap_or_default()
                        ),
                        details,
                        folder: None,
                    }
                }
                Err(e) => JobResult {
                    job: Job::Bench,
                    passed: false,
                    headline: "benchmark failed".into(),
                    details: e,
                    folder: None,
                },
            }
        });
    }

    fn target(&self, session: &mut EditorSession) -> Option<GameTarget> {
        GameTarget::for_project(&session.project)
            .map_err(|e| session.error(e))
            .ok()
    }
}

/// A headless run of the open scene file.
fn headless_run(session: &mut EditorSession, script: Option<PathBuf>) -> Option<HeadlessRun> {
    let abs = |p: &std::path::Path| std::path::absolute(p).map_err(|e| e.to_string());
    let run = (|| {
        Ok::<_, String>(HeadlessRun {
            project_root: abs(&session.project.root)?,
            scene: Some(abs(&session.scene_path)?),
            script: script.as_deref().map(abs).transpose()?,
            ticks: 300,
        })
    })();
    run.map_err(|e| session.error(e)).ok()
}

impl Drop for GameTools {
    fn drop(&mut self) {
        if let Some(mut child) = self.play.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn classify(line: &str) -> Option<LogLevel> {
    let l = strip_ansi(line);
    if l.contains(" ERROR ") || l.starts_with("error") {
        Some(LogLevel::Error)
    } else if l.contains(" WARN ") || l.starts_with("warning") {
        Some(LogLevel::Warning)
    } else {
        None
    }
}

/// Removes terminal colour codes from child output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Inspectors

/// A collider that fits the entity's mesh, for Add component > Collider.
pub fn default_collider(entity: &Entity) -> ColliderComponent {
    let boxed = |size: Vec3| ColliderComponent::new(ColliderShape::Box { size });
    match entity.mesh.as_ref().map(|m| &m.source) {
        Some(MeshSource::Primitive { shape, .. }) => match shape {
            PrimitiveShape::Cube => boxed(Vec3::ONE),
            PrimitiveShape::Sphere => ColliderComponent::new(ColliderShape::Sphere { radius: 0.5 }),
            PrimitiveShape::Plane => ColliderComponent {
                offset: Vec3::new(0.0, -0.1, 0.0),
                ..boxed(Vec3::new(1.0, 0.2, 1.0))
            },
            PrimitiveShape::Quad => boxed(Vec3::new(1.0, 1.0, 0.05)),
        },
        // Mesh colliders are static only; a moving model gets a box.
        Some(MeshSource::Model { .. }) if entity.rigidbody.is_none() => {
            ColliderComponent::new(ColliderShape::Mesh)
        }
        Some(MeshSource::Model { .. }) => boxed(Vec3::ONE),
        None => boxed(Vec3::ONE),
    }
}

fn vec3_row(ui: &mut egui::Ui, label: &str, v: Vec3, speed: f64) -> (Vec3, bool, bool) {
    let mut a = v.to_array();
    let (mut changed, mut done) = (false, false);
    ui.horizontal(|ui| {
        ui.label(label);
        for (c, axis) in a.iter_mut().zip(["x", "y", "z"]) {
            let r = ui.add(
                egui::DragValue::new(c)
                    .speed(speed)
                    .prefix(format!("{axis} "))
                    .max_decimals(3),
            );
            changed |= r.changed();
            done |= r.drag_stopped() || r.lost_focus();
        }
    });
    (Vec3::from(a), changed, done)
}

pub fn collider_editor(ui: &mut egui::Ui, session: &mut EditorSession, entity: &Entity) {
    let id = entity.id;
    let Some(original) = entity.collider else {
        return;
    };
    let mut c = original;
    let mut done = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("collider-shape")
            .selected_text(c.shape.label())
            .show_ui(ui, |ui| {
                let options = [
                    ColliderShape::Box { size: Vec3::ONE },
                    ColliderShape::Sphere { radius: 0.5 },
                    ColliderShape::Capsule {
                        radius: 0.3,
                        height: 1.8,
                    },
                    ColliderShape::Mesh,
                ];
                for o in options {
                    let allowed = o != ColliderShape::Mesh || entity.rigidbody.is_none();
                    let r = ui
                        .add_enabled(
                            allowed,
                            egui::Button::selectable(c.shape.label() == o.label(), o.label()),
                        )
                        .on_disabled_hover_text(
                            "Mesh colliders are static only: remove the rigidbody first",
                        );
                    if r.clicked() {
                        c.shape = o;
                        done = true;
                    }
                }
            });
        done |= ui
            .checkbox(&mut c.sensor, "sensor")
            .on_hover_text("Reports overlaps (triggers) but never blocks or pushes anything")
            .changed();
    });
    let mut drag = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64| {
        ui.horizontal(|ui| {
            ui.label(label);
            let r = ui.add(egui::DragValue::new(v).speed(speed).max_decimals(3));
            done |= r.drag_stopped() || r.lost_focus();
        });
    };
    match &mut c.shape {
        ColliderShape::Box { size } => {
            let (v, _, d) = vec3_row(ui, "Size", *size, 0.01);
            *size = v.max(Vec3::splat(0.001));
            done |= d;
        }
        ColliderShape::Sphere { radius } => {
            drag(ui, "Radius", radius, 0.01);
            *radius = radius.max(0.001);
        }
        ColliderShape::Capsule { radius, height } => {
            drag(ui, "Radius", radius, 0.01);
            drag(ui, "Height", height, 0.01);
            *radius = radius.max(0.001);
            *height = height.max(0.001);
        }
        ColliderShape::Mesh => {
            ui.label(egui::RichText::new("uses the entity's mesh triangles (static only)").small());
        }
    }
    let (offset, _, d) = vec3_row(ui, "Offset", c.offset, 0.01);
    c.offset = offset;
    done |= d;
    ui.horizontal(|ui| {
        ui.label("Layer");
        let r = ui.add(egui::DragValue::new(&mut c.layer).range(0..=15));
        done |= r.drag_stopped() || r.lost_focus();
        ui.label("Friction");
        let r = ui.add(
            egui::DragValue::new(&mut c.friction)
                .speed(0.01)
                .range(0.0..=10.0),
        );
        done |= r.drag_stopped() || r.lost_focus();
        ui.label("Bounciness");
        let r = ui
            .add(
                egui::DragValue::new(&mut c.restitution)
                    .speed(0.01)
                    .range(0.0..=1.0),
            )
            .on_hover_text("0 = no bounce, 1 = bounces back at full speed");
        done |= r.drag_stopped() || r.lost_focus();
    });
    let key = format!("collider:{id}");
    if c != original {
        session.execute_merging(
            Command::SetCollider {
                id,
                collider: Some(c),
            },
            Some(&key),
        );
    }
    if done {
        session.end_merge();
    }
}

/// The rigidbody card's fields (the card header adds and removes it).
pub fn rigidbody_editor(ui: &mut egui::Ui, session: &mut EditorSession, entity: &Entity) {
    let id = entity.id;
    let Some(original) = entity.rigidbody else {
        return;
    };
    let mut rb = original;
    let mut done = false;
    ui.horizontal(|ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt("rigidbody-kind")
            .selected_text(rb.kind().label())
            .show_ui(ui, |ui| {
                for (kinematic, kind, help) in [
                    (
                        false,
                        BodyKind::Dynamic,
                        "Simulated: falls, collides and can be pushed",
                    ),
                    (
                        true,
                        BodyKind::Kinematic,
                        "Moved by its transform (doors, lifts); pushes others",
                    ),
                ] {
                    if ui
                        .selectable_label(rb.kinematic == kinematic, kind.label())
                        .on_hover_text(help)
                        .clicked()
                    {
                        rb.kinematic = kinematic;
                        done = true;
                    }
                }
            });
    });
    if entity.collider.is_none() {
        ui.label(
            egui::RichText::new("No collider: it falls but touches nothing. Add a collider.")
                .small()
                .color(Color32::from_rgb(230, 180, 80)),
        );
    }
    if !rb.kinematic {
        let mut drag =
            |ui: &mut egui::Ui, label: &str, help: &str, v: &mut f32, speed: f64, range| {
                ui.horizontal(|ui| {
                    ui.label(label).on_hover_text(help);
                    let r = ui.add(
                        egui::DragValue::new(v)
                            .speed(speed)
                            .range(range)
                            .max_decimals(3),
                    );
                    done |= r.drag_stopped() || r.lost_focus();
                });
            };
        drag(
            ui,
            "Mass (kg)",
            "Heavier bodies are harder to push",
            &mut rb.mass,
            0.05,
            0.001..=100_000.0,
        );
        drag(
            ui,
            "Gravity scale",
            "1 = normal gravity, 0 = floats, negative = falls upwards",
            &mut rb.gravity_scale,
            0.01,
            -copperframe_scene::components::MAX_GRAVITY_SCALE
                ..=copperframe_scene::components::MAX_GRAVITY_SCALE,
        );
        drag(
            ui,
            "Linear damping",
            "Air drag: slows movement over time",
            &mut rb.linear_damping,
            0.01,
            0.0..=copperframe_scene::components::MAX_DAMPING,
        );
        drag(
            ui,
            "Angular damping",
            "Slows spinning over time",
            &mut rb.angular_damping,
            0.01,
            0.0..=copperframe_scene::components::MAX_DAMPING,
        );
        done |= ui
            .checkbox(&mut rb.lock_rotation, "Lock rotation")
            .on_hover_text("Keep it upright: collisions never tip it over")
            .changed();
    }
    let key = format!("rigidbody:{id}");
    if rb != original {
        session.execute_merging(
            Command::SetRigidbody {
                id,
                rigidbody: Some(rb),
            },
            Some(&key),
        );
    }
    if done {
        session.end_merge();
    }
}

pub fn audio_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    entity: &Entity,
    sounds: &[(AssetId, String)],
) {
    let id = entity.id;
    let Some(original) = entity.audio else {
        return;
    };
    let mut a = original;
    let mut done = false;
    let label = sounds
        .iter()
        .find(|(s, _)| *s == a.sound)
        .map_or_else(|| format!("missing {}", a.sound), |(_, p)| p.clone());
    egui::ComboBox::from_id_salt("audio-sound")
        .selected_text(label)
        .show_ui(ui, |ui| {
            for (asset, path) in sounds {
                if ui.selectable_label(a.sound == *asset, path).clicked() {
                    a.sound = *asset;
                    done = true;
                }
            }
        });
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("audio-bus")
            .selected_text(match a.bus {
                AudioBus::Effects => "Effects",
                AudioBus::Music => "Music",
            })
            .show_ui(ui, |ui| {
                done |= ui
                    .selectable_value(&mut a.bus, AudioBus::Effects, "Effects")
                    .changed();
                done |= ui
                    .selectable_value(&mut a.bus, AudioBus::Music, "Music")
                    .changed();
            });
        done |= ui.checkbox(&mut a.looping, "loop").changed();
        done |= ui.checkbox(&mut a.autoplay, "autoplay").changed();
        done |= ui.checkbox(&mut a.spatial, "3D").changed();
    });
    ui.horizontal(|ui| {
        ui.label("Volume dB");
        let r = ui.add(
            egui::DragValue::new(&mut a.volume_db)
                .speed(0.1)
                .range(-60.0..=12.0),
        );
        done |= r.drag_stopped() || r.lost_focus();
    });
    if a.spatial {
        ui.horizontal(|ui| {
            ui.label("Distance");
            let r1 = ui.add(
                egui::DragValue::new(&mut a.min_distance)
                    .speed(0.05)
                    .range(0.01..=1000.0)
                    .prefix("min "),
            );
            let r2 = ui.add(
                egui::DragValue::new(&mut a.max_distance)
                    .speed(0.1)
                    .range(0.02..=10_000.0)
                    .prefix("max "),
            );
            done |= r1.drag_stopped() || r1.lost_focus() || r2.drag_stopped() || r2.lost_focus();
        });
    }
    let key = format!("audio:{id}");
    if a != original {
        session.execute_merging(Command::SetAudio { id, audio: Some(a) }, Some(&key));
    }
    if done {
        session.end_merge();
    }
}

fn descriptor<'a>(schema: Option<&'a Value>, kind: &str) -> Option<&'a Value> {
    schema?["behaviors"]
        .as_array()?
        .iter()
        .find(|b| b["kind"] == kind)
}

/// The behaviour kinds the game registers, for the Add component menu:
/// `(kind, folder, description)`.
pub fn behavior_kinds(schema: &Value) -> Vec<(String, String, String)> {
    schema["behaviors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            Some((
                b["kind"].as_str()?.to_owned(),
                b["folder"].as_str().unwrap_or("").to_owned(),
                b["description"].as_str().unwrap_or("").to_owned(),
            ))
        })
        .collect()
}

/// The fields of the entity's behaviour of `kind`. `schema` is the game's
/// behaviour schema when known.
pub fn behavior_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    tools: &mut GameTools,
    entity: &Entity,
    kind: &str,
) {
    let id = entity.id;
    let Some(i) = entity.behaviors.iter().position(|b| b.kind == kind) else {
        return;
    };
    let schema = tools.schema().cloned();
    let original = entity.behaviors.clone();
    let mut list = original.clone();
    let mut done = false;
    let names: Vec<(EntityId, String)> = session
        .scene
        .entities
        .iter()
        .map(|e| (e.id, e.name.clone()))
        .collect();
    let b = &mut list[i];
    let Some(d) = descriptor(schema.as_ref(), &b.kind) else {
        if schema.is_some() {
            ui.colored_label(
                Color32::from_rgb(235, 90, 80),
                "not registered by this game: the scene will not play",
            );
        } else if tools.busy_label().is_some() {
            ui.label(egui::RichText::new("reading the game's behaviour kinds…").small());
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("fields unknown until the game is built.").small());
                if ui
                    .small_button("Load behaviour kinds")
                    .on_hover_text("Builds the game and reads its behaviour kinds")
                    .clicked()
                {
                    tools.refresh_schema(session);
                }
            });
        }
        for (k, v) in &b.params {
            ui.label(format!("{k} = {v}"));
        }
        return;
    };
    if let Some(text) = d["description"].as_str() {
        ui.label(egui::RichText::new(text).small().weak());
    }
    for f in d["fields"].as_array().into_iter().flatten() {
        let Some(name) = f["name"].as_str() else {
            continue;
        };
        let default = &f["default"];
        let current = b
            .params
            .get(name)
            .cloned()
            .unwrap_or_else(|| default.clone());
        let help = f["help"].as_str().unwrap_or("");
        let mut new = None;
        ui.horizontal(|ui| {
            ui.label(name).on_hover_text(help);
            match f["kind"].as_str() {
                Some("number") => {
                    let mut v = current.as_f64().unwrap_or(0.0);
                    let r = ui.add(egui::DragValue::new(&mut v).speed(0.01).max_decimals(3));
                    if r.changed() {
                        new = Some(Value::from(v));
                    }
                    done |= r.drag_stopped() || r.lost_focus();
                }
                Some("bool") => {
                    let mut v = current.as_bool().unwrap_or(false);
                    if ui.checkbox(&mut v, "").changed() {
                        new = Some(Value::from(v));
                        done = true;
                    }
                }
                Some("text") => {
                    let mut v = current.as_str().unwrap_or("").to_owned();
                    let r = ui.text_edit_singleline(&mut v);
                    if r.changed() {
                        new = Some(Value::from(v));
                    }
                    done |= r.lost_focus();
                }
                Some("entity") => {
                    let cur: Option<EntityId> = current.as_str().and_then(|s| s.parse().ok());
                    let label = cur
                        .and_then(|c| names.iter().find(|(i, _)| *i == c))
                        .map_or("(none)".to_owned(), |(_, n)| n.clone());
                    egui::ComboBox::from_id_salt(("behaviour-entity", kind, name))
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(cur.is_none(), "(none)").clicked() {
                                new = Some(Value::from(""));
                            }
                            for (eid, n) in &names {
                                if ui.selectable_label(cur == Some(*eid), n).clicked() {
                                    new = Some(Value::from(eid.to_string()));
                                }
                            }
                        });
                    done |= new.is_some();
                }
                _ => {
                    ui.label(current.to_string());
                }
            }
        });
        if let Some(v) = new {
            if &v == default {
                b.params.remove(name);
            } else {
                b.params.insert(name.to_owned(), v);
            }
        }
    }
    let key = format!("behaviour:{id}:{kind}");
    if list != original {
        session.execute_merging(
            Command::SetBehaviors {
                id,
                behaviors: list,
            },
            Some(&key),
        );
    }
    if done {
        session.end_merge();
    }
}

/// Sound assets for the audio inspector.
pub fn sounds(session: &EditorSession) -> Vec<(AssetId, String)> {
    session
        .db
        .entries
        .iter()
        .filter(|e| e.kind == AssetKind::Audio)
        .map(|e| (e.id, e.relative.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Collider display

/// Wireframe colour for a collider on a body of kind `body`.
pub fn collider_color(c: &ColliderComponent, body: BodyKind) -> Color32 {
    if c.sensor {
        return Color32::from_rgb(240, 220, 60);
    }
    match body {
        BodyKind::Static => Color32::from_rgb(90, 220, 120),
        BodyKind::Dynamic => Color32::from_rgb(250, 150, 50),
        BodyKind::Kinematic => Color32::from_rgb(90, 170, 255),
    }
}

/// Line segments (in world space) outlining a collider whose entity has
/// world matrix `world`. Mesh colliders use `mesh_bounds` (local).
pub fn collider_lines(
    c: &ColliderComponent,
    world: glam::Mat4,
    mesh_bounds: Option<(Vec3, Vec3)>,
) -> Vec<(Vec3, Vec3)> {
    let mut lines = Vec::new();
    let p = |v: Vec3| world.transform_point3(v + c.offset);
    let box_lines = |lo: Vec3, hi: Vec3, lines: &mut Vec<(Vec3, Vec3)>| {
        let corner = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        };
        for (a, b) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            lines.push((p(corner(a)), p(corner(b))));
        }
    };
    let circle = |center: Vec3, u: Vec3, v: Vec3, r: f32, lines: &mut Vec<(Vec3, Vec3)>| {
        const N: usize = 24;
        let at = |i: usize| {
            let t = i as f32 / N as f32 * std::f32::consts::TAU;
            center + (u * t.cos() + v * t.sin()) * r
        };
        for i in 0..N {
            lines.push((p(at(i)), p(at(i + 1))));
        }
    };
    match c.shape {
        ColliderShape::Box { size } => box_lines(-size * 0.5, size * 0.5, &mut lines),
        ColliderShape::Sphere { radius } => {
            circle(Vec3::ZERO, Vec3::X, Vec3::Y, radius, &mut lines);
            circle(Vec3::ZERO, Vec3::X, Vec3::Z, radius, &mut lines);
            circle(Vec3::ZERO, Vec3::Y, Vec3::Z, radius, &mut lines);
        }
        ColliderShape::Capsule { radius, height } => {
            // Total height along local Y, like the physics capsule.
            let half = (height * 0.5 - radius).max(0.0);
            for y in [-half, half] {
                circle(Vec3::Y * y, Vec3::X, Vec3::Z, radius, &mut lines);
            }
            circle(Vec3::ZERO, Vec3::X, Vec3::Y, radius, &mut lines);
            for d in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z] {
                lines.push((
                    p(d * radius - Vec3::Y * half),
                    p(d * radius + Vec3::Y * half),
                ));
            }
        }
        ColliderShape::Mesh => {
            if let Some((lo, hi)) = mesh_bounds {
                box_lines(lo, hi, &mut lines);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_codes_are_stripped_and_levels_detected() {
        let line = "\u{1b}[2m2026\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m psx: careful";
        assert_eq!(strip_ansi(line), "2026  WARN psx: careful");
        assert_eq!(classify(line), Some(LogLevel::Warning));
        assert_eq!(classify("   Compiling x"), None);
    }

    #[test]
    fn collider_wireframes_follow_offset_and_transform() {
        let c = ColliderComponent {
            offset: Vec3::new(0.0, 1.0, 0.0),
            ..ColliderComponent::new(ColliderShape::Box {
                size: Vec3::splat(2.0),
            })
        };
        let world = glam::Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let lines = collider_lines(&c, world, None);
        assert_eq!(lines.len(), 12);
        let ys: Vec<f32> = lines.iter().flat_map(|(a, b)| [a.y, b.y]).collect();
        assert!(
            ys.iter()
                .all(|y| (*y - 0.0).abs() < 1e-5 || (*y - 2.0).abs() < 1e-5)
        );
        assert!(lines.iter().all(|(a, _)| (a.x - 10.0).abs() <= 1.0 + 1e-5));
        let sphere = ColliderComponent::new(ColliderShape::Sphere { radius: 0.5 });
        assert_eq!(
            collider_lines(&sphere, glam::Mat4::IDENTITY, None).len(),
            72
        );
        assert!(
            collider_lines(
                &ColliderComponent::new(ColliderShape::Mesh),
                glam::Mat4::IDENTITY,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn default_colliders_fit_primitives() {
        let mut e = Entity::new("p");
        e.mesh = Some(copperframe_scene::MeshComponent {
            source: MeshSource::Primitive {
                shape: PrimitiveShape::Plane,
                texture: None,
                tile_size: None,
            },
            material: Default::default(),
        });
        let c = default_collider(&e);
        assert!(c.problem().is_none());
        assert_eq!(c.offset.y, -0.1);
    }
}
