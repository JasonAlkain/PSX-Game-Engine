//! Gameplay authoring in the editor: inspectors for colliders, audio
//! sources and behaviours, Play/Stop (the game runs as a child process on a
//! temporary copy of the scene), the behaviour schema, and export.
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
use copperframe_assets::export::{ExportOptions, export, verify_export};
use copperframe_assets::game::{GameTarget, cached_schema, write_schema_cache};
use copperframe_core::{AssetId, EntityId};
use copperframe_scene::{
    AudioBus, AudioSourceComponent, BehaviorComponent, BodyKind, ColliderComponent, ColliderShape,
    Command, Entity, MeshSource, PrimitiveShape,
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
    ExportDone(Result<String, String>),
}

/// Child processes and background jobs started from the editor.
pub struct GameTools {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    play: Option<Child>,
    schema: Option<Value>,
    schema_busy: bool,
    export_busy: bool,
    pub export_dir: String,
}

impl GameTools {
    pub fn new(session: &EditorSession) -> Self {
        let (tx, rx) = channel();
        let export_dir = session
            .project
            .root
            .join("dist")
            .join(file_slug(&session.project.file.name))
            .display()
            .to_string();
        let mut tools = Self {
            tx,
            rx,
            play: None,
            schema: cached_schema(&session.project),
            schema_busy: false,
            export_busy: false,
            export_dir,
        };
        if tools.schema.is_none() {
            tools.refresh_schema(session);
        }
        tools
    }

    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    pub fn busy_label(&self) -> Option<&'static str> {
        if self.export_busy {
            Some("exporting…")
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
                Msg::ExportDone(result) => {
                    self.export_busy = false;
                    match result {
                        Ok(text) => session.info(text),
                        Err(e) => session.error(format!("export failed: {e}")),
                    }
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
        if self.play.is_some() {
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

    /// Exports to [`GameTools::export_dir`] in the background, replacing an
    /// earlier export there, then runs the export headless as a check.
    /// Unsaved changes are not exported; the caller saves first.
    pub fn export(&mut self, session: &mut EditorSession) {
        if self.export_busy {
            return;
        }
        let target = match GameTarget::for_project(&session.project) {
            Ok(t) => t,
            Err(e) => {
                session.error(e);
                return;
            }
        };
        let notices_manifest = match &target {
            GameTarget::Crate { manifest } => manifest.clone(),
            GameTarget::Executable { .. } => {
                session.error(
                    "export of projects without a game crate is done with `copperframe build` \
                     (it needs the engine source to list licences)",
                );
                return;
            }
        };
        let project = session.project.clone();
        let out = PathBuf::from(self.export_dir.trim());
        let tx = self.tx.clone();
        self.export_busy = true;
        session.info(format!("exporting to {} …", out.display()));
        std::thread::spawn(move || {
            let options = ExportOptions {
                out,
                replace: true,
                notices_manifest,
            };
            let log_tx = tx.clone();
            let mut log = |line: &str| {
                let _ = log_tx.send(Msg::Log(
                    LogLevel::Info,
                    format!("[export] {}", strip_ansi(line)),
                ));
            };
            let result = export(&project, &target, &options, &mut log).and_then(|r| {
                verify_export(&r.executable, None, 120).map(|_| {
                    format!(
                        "exported to {} ({} files, {:.1} MiB) and checked that it starts",
                        r.out.display(),
                        r.files,
                        r.bytes as f64 / (1024.0 * 1024.0)
                    )
                })
            });
            let _ = tx.send(Msg::ExportDone(result));
        });
    }
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

fn file_slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_owned();
    if s.is_empty() { "game".into() } else { s }
}

// ---------------------------------------------------------------------------
// Inspectors

/// A collider that fits the entity's mesh, for "+ Collider".
pub fn default_collider(entity: &Entity) -> ColliderComponent {
    let boxed = |size: Vec3| ColliderComponent::new(ColliderShape::Box { size }, BodyKind::Static);
    match entity.mesh.as_ref().map(|m| &m.source) {
        Some(MeshSource::Primitive { shape, .. }) => match shape {
            PrimitiveShape::Cube => boxed(Vec3::ONE),
            PrimitiveShape::Sphere => {
                ColliderComponent::new(ColliderShape::Sphere { radius: 0.5 }, BodyKind::Static)
            }
            PrimitiveShape::Plane => ColliderComponent {
                offset: Vec3::new(0.0, -0.1, 0.0),
                ..boxed(Vec3::new(1.0, 0.2, 1.0))
            },
            PrimitiveShape::Quad => boxed(Vec3::new(1.0, 1.0, 0.05)),
        },
        Some(MeshSource::Model { .. }) => {
            ColliderComponent::new(ColliderShape::Mesh, BodyKind::Static)
        }
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
    ui.label(egui::RichText::new("Collider").strong());
    let Some(original) = entity.collider else {
        if ui.button("+ Collider").clicked() {
            session.execute(Command::SetCollider {
                id,
                collider: Some(default_collider(entity)),
            });
        }
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
                    if ui
                        .selectable_label(c.shape.label() == o.label(), o.label())
                        .clicked()
                    {
                        c.shape = o;
                        done = true;
                    }
                }
            });
        egui::ComboBox::from_id_salt("collider-body")
            .selected_text(c.body.label())
            .show_ui(ui, |ui| {
                for b in BodyKind::ALL {
                    if ui.selectable_label(c.body == b, b.label()).clicked() {
                        c.body = b;
                        done = true;
                    }
                }
            });
        done |= ui.checkbox(&mut c.sensor, "sensor").changed();
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
        if c.body == BodyKind::Dynamic {
            ui.label("Mass");
            let r = ui.add(
                egui::DragValue::new(&mut c.mass)
                    .speed(0.05)
                    .range(0.001..=100_000.0),
            );
            done |= r.drag_stopped() || r.lost_focus();
        }
        ui.label("Friction");
        let r = ui.add(
            egui::DragValue::new(&mut c.friction)
                .speed(0.01)
                .range(0.0..=10.0),
        );
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
    if ui.button("Remove collider").clicked() {
        session.execute(Command::SetCollider { id, collider: None });
    }
}

pub fn audio_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    entity: &Entity,
    sounds: &[(AssetId, String)],
) {
    let id = entity.id;
    ui.label(egui::RichText::new("Audio source").strong());
    let Some(original) = entity.audio else {
        if sounds.is_empty() {
            ui.label(
                egui::RichText::new("add .wav or .ogg files to assets/ to use sounds").small(),
            );
            return;
        }
        egui::ComboBox::from_id_salt("add-audio")
            .selected_text("+ Audio source…")
            .show_ui(ui, |ui| {
                for (asset, path) in sounds {
                    if ui.selectable_label(false, path).clicked() {
                        session.execute(Command::SetAudio {
                            id,
                            audio: Some(AudioSourceComponent::new(*asset)),
                        });
                    }
                }
            });
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
    if ui.button("Remove audio source").clicked() {
        session.execute(Command::SetAudio { id, audio: None });
    }
}

fn descriptor<'a>(schema: Option<&'a Value>, kind: &str) -> Option<&'a Value> {
    schema?["behaviors"]
        .as_array()?
        .iter()
        .find(|b| b["kind"] == kind)
}

pub fn behaviors_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    tools: &mut GameTools,
    entity: &Entity,
) {
    let id = entity.id;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Behaviours").strong());
        if ui
            .small_button("Reload")
            .on_hover_text("Reload behaviour kinds from the game")
            .clicked()
        {
            tools.refresh_schema(session);
        }
    });
    let schema = tools.schema().cloned();
    if schema.is_none() {
        ui.label(
            egui::RichText::new(if tools.busy_label().is_some() {
                "reading the game's behaviour kinds…"
            } else {
                "behaviour kinds unknown: press Reload (builds the game)"
            })
            .small(),
        );
    }
    let original = entity.behaviors.clone();
    let mut list = original.clone();
    let mut remove = None;
    let mut done = false;
    let names: Vec<(EntityId, String)> = session
        .scene
        .entities
        .iter()
        .map(|e| (e.id, e.name.clone()))
        .collect();
    for (i, b) in list.iter_mut().enumerate() {
        let d = descriptor(schema.as_ref(), &b.kind);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&b.kind).monospace());
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
            let Some(d) = d else {
                if schema.is_some() {
                    ui.colored_label(
                        Color32::from_rgb(235, 90, 80),
                        "not registered by this game: the scene will not play",
                    );
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
                            let r =
                                ui.add(egui::DragValue::new(&mut v).speed(0.01).max_decimals(3));
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
                            let cur: Option<EntityId> =
                                current.as_str().and_then(|s| s.parse().ok());
                            let label = cur
                                .and_then(|c| names.iter().find(|(i, _)| *i == c))
                                .map_or("(none)".to_owned(), |(_, n)| n.clone());
                            egui::ComboBox::from_id_salt(("behaviour-entity", i, name))
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
        });
    }
    if let Some(i) = remove {
        list.remove(i);
        done = true;
    }
    // Add a kind the entity does not have yet.
    if let Some(kinds) = schema.as_ref().and_then(|s| s["behaviors"].as_array()) {
        let available: Vec<&str> = kinds
            .iter()
            .filter_map(|k| k["kind"].as_str())
            .filter(|k| !list.iter().any(|b| b.kind == *k))
            .collect();
        if !available.is_empty() {
            egui::ComboBox::from_id_salt("add-behaviour")
                .selected_text("+ Behaviour…")
                .show_ui(ui, |ui| {
                    for k in available {
                        if ui.selectable_label(false, k).clicked() {
                            list.push(BehaviorComponent::new(k));
                            done = true;
                        }
                    }
                });
        }
    }
    let key = format!("behaviours:{id}");
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

/// Wireframe colour for a collider.
pub fn collider_color(c: &ColliderComponent) -> Color32 {
    if c.sensor {
        return Color32::from_rgb(240, 220, 60);
    }
    match c.body {
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
            ..ColliderComponent::new(
                ColliderShape::Box {
                    size: Vec3::splat(2.0),
                },
                BodyKind::Static,
            )
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
        let sphere =
            ColliderComponent::new(ColliderShape::Sphere { radius: 0.5 }, BodyKind::Dynamic);
        assert_eq!(
            collider_lines(&sphere, glam::Mat4::IDENTITY, None).len(),
            72
        );
        assert!(
            collider_lines(
                &ColliderComponent::new(ColliderShape::Mesh, BodyKind::Static),
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
            },
            material: Default::default(),
        });
        let c = default_collider(&e);
        assert!(c.problem().is_none());
        assert_eq!(c.offset.y, -0.1);
    }
}
