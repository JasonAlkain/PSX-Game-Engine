//! egui panels: menu bar, hierarchy, inspector, scene settings, assets,
//! console, and the viewport's selection outline and translate gizmo.
//!
//! Panels never mutate the scene directly; they build [`Command`]s and run
//! them through the [`EditorSession`], so every change is validated and
//! undoable. Continuous edits (drags, typing) merge into one undo step per
//! gesture.

use copperframe_assets::AssetKind;
use copperframe_core::{AssetId, EntityId, RetroStyle, TextureFilter};
use copperframe_render::Rect;
use copperframe_runtime::scene_gpu::{AssetStatus, SceneResources};
use copperframe_scene::{
    AlphaSetting, Command, MeshComponent, MeshSource, PrimitiveShape, quat_to_euler_deg,
};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, Pos2, Sense, Stroke};
use glam::{Mat4, Vec2, Vec3};

use crate::gameplay::{self, GameTools};
use crate::session::{EditorSession, LogLevel, SaveOutcome};
use crate::viewport::{AXES, EditorCamera, ViewportGeometry, axis_drag_distance, snap};
use crate::visuals;

/// Requests the app must carry out (they need the GPU or the event loop).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Reimport(AssetId),
    /// Asset database was rescanned; retry assets that failed to load.
    AssetsChanged,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Assets,
    Console,
}

#[derive(Debug, Clone, Copy)]
struct GizmoDrag {
    entity: EntityId,
    axis: usize,
    start_world: Vec3,
    pixels: Vec2,
}

/// Frame statistics shown in the status bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub fps: f32,
    pub draws: usize,
    pub triangles: u64,
}

/// UI state that is not part of the scene.
pub struct UiState {
    bottom: BottomTab,
    snap_enabled: bool,
    snap_translate: f32,
    snap_rotate: f32,
    snap_scale: f32,
    keep_world_on_reparent: bool,
    show_colliders: bool,
    name_buffer: Option<(EntityId, String)>,
    gizmo: Option<GizmoDrag>,
    confirm_close: bool,
    conflict: Option<u64>,
    pub stats: Stats,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            bottom: BottomTab::Console,
            snap_enabled: false,
            snap_translate: 0.25,
            snap_rotate: 15.0,
            snap_scale: 0.1,
            keep_world_on_reparent: true,
            show_colliders: true,
            name_buffer: None,
            gizmo: None,
            confirm_close: false,
            conflict: None,
            stats: Stats::default(),
        }
    }
}

impl UiState {
    /// Asks before closing when there are unsaved changes. Returns true if
    /// the app may quit now.
    pub fn request_close(&mut self, session: &EditorSession) -> bool {
        if session.is_dirty() {
            self.confirm_close = true;
            false
        } else {
            true
        }
    }
}

/// What the frame's UI produced.
pub struct UiOutput {
    /// The viewport panel in window pixels.
    pub viewport: Option<Rect>,
    pub actions: Vec<Action>,
}

/// Everything the panels read or change.
pub struct UiContext<'a> {
    pub session: &'a mut EditorSession,
    pub tools: &'a mut GameTools,
    pub state: &'a mut UiState,
    pub camera: &'a mut EditorCamera,
    pub resources: &'a SceneResources,
    pub dt: f32,
}

pub fn draw(ui: &mut egui::Ui, cx: UiContext<'_>) -> UiOutput {
    let UiContext {
        session,
        tools,
        state,
        camera,
        resources,
        dt,
    } = cx;
    let mut actions = Vec::new();
    shortcuts(ui.ctx(), session, state, tools);

    egui::Panel::top("menu").show(ui, |ui| {
        menu_bar(ui, session, state, camera, tools, &mut actions);
    });
    egui::Panel::bottom("status")
        .resizable(false)
        .show(ui, |ui| status_bar(ui, session, state));
    egui::Panel::bottom("bottom")
        .resizable(true)
        .default_size(170.0)
        .show(ui, |ui| {
            bottom_panel(ui, session, state, camera, resources, &mut actions)
        });
    egui::Panel::left("hierarchy")
        .resizable(true)
        .default_size(230.0)
        .show(ui, |ui| hierarchy(ui, session));
    egui::Panel::right("inspector")
        .resizable(true)
        .default_size(320.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                inspector(ui, session, state, resources, tools);
                ui.separator();
                scene_settings(ui, session, camera);
            });
        });
    let ppp = ui.ctx().pixels_per_point();
    let viewport = egui::CentralPanel::no_frame()
        .show(ui, |ui| viewport(ui, session, state, camera, resources, dt))
        .inner
        .map(|r| Rect {
            x: (r.min.x * ppp).round().max(0.0) as u32,
            y: (r.min.y * ppp).round().max(0.0) as u32,
            width: (r.width() * ppp).round().max(0.0) as u32,
            height: (r.height() * ppp).round().max(0.0) as u32,
        });

    modals(ui.ctx(), session, state, &mut actions);
    UiOutput { viewport, actions }
}

fn shortcuts(
    ctx: &egui::Context,
    session: &mut EditorSession,
    state: &mut UiState,
    tools: &mut GameTools,
) {
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
        if tools.is_playing() {
            tools.stop(session);
        } else {
            tools.play(session);
        }
    }
    let cmd = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
    let cmd_shift = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
    if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
        save(session, state, false);
    }
    // Text fields handle their own undo while focused.
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::Z)) || i.consume_shortcut(&cmd(Key::Y)))
    {
        session.redo();
    } else if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::Z))) {
        session.undo();
    }
    if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::D))) {
        session.duplicate_selected();
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete)) {
        session.delete_selected();
    }
}

fn save(session: &mut EditorSession, state: &mut UiState, force: bool) {
    match session.save(force) {
        Ok(SaveOutcome::Saved { .. }) => state.conflict = None,
        Ok(SaveOutcome::Conflict { disk_revision }) => state.conflict = Some(disk_revision),
        Err(_) => {} // logged by the session
    }
}

fn menu_bar(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &EditorCamera,
    tools: &mut GameTools,
    actions: &mut Vec<Action>,
) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("Save   Ctrl+S").clicked() {
                save(session, state, false);
            }
            if ui.button("Reload from disk (discard changes)").clicked()
                && let Err(e) = session.reload()
            {
                session.error(e.to_string());
            }
            ui.separator();
            if ui.button("Quit").clicked() && state.request_close(session) {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            let undo = session
                .history
                .undo_label()
                .map_or("Undo".to_owned(), |l| format!("Undo {l}"));
            if ui
                .add_enabled(session.history.can_undo(), egui::Button::new(format!("{undo}   Ctrl+Z")))
                .clicked()
            {
                session.undo();
            }
            let redo = session
                .history
                .redo_label()
                .map_or("Redo".to_owned(), |l| format!("Redo {l}"));
            if ui
                .add_enabled(session.history.can_redo(), egui::Button::new(format!("{redo}   Ctrl+Y")))
                .clicked()
            {
                session.redo();
            }
            ui.separator();
            let has = session.selection.is_some();
            if ui.add_enabled(has, egui::Button::new("Duplicate   Ctrl+D")).clicked() {
                session.duplicate_selected();
            }
            if ui.add_enabled(has, egui::Button::new("Delete   Del")).clicked() {
                session.delete_selected();
            }
            ui.separator();
            ui.checkbox(&mut state.show_colliders, "Show colliders");
            ui.checkbox(&mut state.snap_enabled, "Snap");
            ui.add(egui::DragValue::new(&mut state.snap_translate).speed(0.01).range(0.01..=10.0).prefix("move "));
            ui.add(egui::DragValue::new(&mut state.snap_rotate).speed(0.5).range(1.0..=90.0).prefix("rotate ").suffix("°"));
            ui.add(egui::DragValue::new(&mut state.snap_scale).speed(0.01).range(0.01..=10.0).prefix("scale "));
        });
        ui.menu_button("Create", |ui| {
            let at = camera.spawn_point(4.0);
            if ui.button("Empty").clicked() {
                session.create_mesh_entity("Empty", None, at);
            }
            for shape in PrimitiveShape::ALL {
                if ui.button(shape.label()).clicked() {
                    session.create_primitive(shape, at);
                }
            }
        });
        ui.menu_button("Game", |ui| {
            if tools.is_playing() {
                if ui.button("Stop   F5").clicked() {
                    tools.stop(session);
                }
            } else if ui.button("Play   F5").clicked() {
                tools.play(session);
            }
            if ui.button("Reload behaviour kinds").clicked() {
                tools.refresh_schema(session);
            }
            ui.separator();
            ui.label("Export folder");
            ui.text_edit_singleline(&mut tools.export_dir);
            let label = if session.is_dirty() {
                "Save and export"
            } else {
                "Export"
            };
            if ui
                .add_enabled(tools.busy_label().is_none(), egui::Button::new(label))
                .on_hover_text(
                    "Release build + project data + licence notices; replaces an earlier \
                     export in that folder, then checks the export starts.",
                )
                .clicked()
            {
                save(session, state, false);
                if session.is_dirty() {
                    session.error("export cancelled: the scene could not be saved");
                } else {
                    tools.export(session);
                }
            }
        });
        // Play/Stop where it is always visible.
        ui.separator();
        if tools.is_playing() {
            if ui
                .button(egui::RichText::new("Stop").color(Color32::from_rgb(235, 110, 90)))
                .clicked()
            {
                tools.stop(session);
            }
        } else if ui
            .button(egui::RichText::new("Play").color(Color32::from_rgb(120, 210, 120)))
            .on_hover_text("Run the game on a copy of this scene (F5)")
            .clicked()
        {
            tools.play(session);
        }
        if let Some(busy) = tools.busy_label() {
            ui.label(egui::RichText::new(busy).weak());
        }
        ui.menu_button("Help", |ui| {
            ui.label(
                "Viewport: left click selects, drag a coloured arrow to move along an axis.\n\
                 Hold the right mouse button to look; WASD/QE fly while held, Shift is faster;\n\
                 the wheel moves forward/back.\n\
                 Ctrl+S save, Ctrl+Z undo, Ctrl+Y / Ctrl+Shift+Z redo, Ctrl+D duplicate, Del delete.\n\
                 F5 plays the scene in a separate game window (unsaved edits included) and stops it.",
            );
        });
    });
}

fn status_bar(ui: &mut egui::Ui, session: &EditorSession, state: &UiState) {
    ui.horizontal(|ui| {
        let dirty = if session.is_dirty() { " * unsaved" } else { "" };
        ui.label(format!(
            "{} — {} (revision {}){dirty}",
            session.project.file.name,
            session.project.relative(&session.scene_path),
            session.scene.revision
        ));
        ui.separator();
        let s = state.stats;
        ui.label(format!(
            "{:.0} fps · {} draws · {} triangles · {} entities",
            s.fps,
            s.draws,
            s.triangles,
            session.scene.entities.len()
        ));
        if let Some(last) = session.log.last()
            && last.level != LogLevel::Info
        {
            ui.separator();
            ui.colored_label(level_color(last.level), &last.text);
        }
    });
}

fn level_color(level: LogLevel) -> Color32 {
    match level {
        LogLevel::Info => Color32::GRAY,
        LogLevel::Warning => Color32::from_rgb(230, 190, 60),
        LogLevel::Error => Color32::from_rgb(240, 90, 80),
    }
}

fn hierarchy(ui: &mut egui::Ui, session: &mut EditorSession) {
    ui.heading("Hierarchy");
    egui::ScrollArea::vertical().show(ui, |ui| {
        fn rows(
            ui: &mut egui::Ui,
            session: &mut EditorSession,
            parent: Option<EntityId>,
            depth: usize,
        ) {
            let kids: Vec<(EntityId, String, bool)> = session
                .scene
                .children(parent)
                .map(|e| (e.id, e.name.clone(), e.mesh.is_some()))
                .collect();
            for (id, name, has_mesh) in kids {
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    let label = if name.is_empty() {
                        "(unnamed)".to_owned()
                    } else {
                        name
                    };
                    // Entities without a mesh (groups) are shown in italics.
                    let mut text = egui::RichText::new(label);
                    if !has_mesh {
                        text = text.italics();
                    }
                    if ui
                        .selectable_label(session.selection == Some(id), text)
                        .clicked()
                    {
                        session.selection = Some(id);
                    }
                });
                if depth < copperframe_scene::scene::MAX_DEPTH {
                    rows(ui, session, Some(id), depth + 1);
                }
            }
        }
        rows(ui, session, None, 0);
    });
}

fn inspector(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    resources: &SceneResources,
    tools: &mut GameTools,
) {
    ui.heading("Inspector");
    let Some(id) = session.selection else {
        ui.label("Nothing selected.");
        return;
    };
    let Some(entity) = session.scene.get(id).cloned() else {
        return;
    };
    ui.label(egui::RichText::new(id.to_string()).small().weak());

    // Name: committed when the field loses focus.
    let buffer = match &mut state.name_buffer {
        Some((bid, text)) if *bid == id => text,
        slot => {
            *slot = Some((id, entity.name.clone()));
            &mut slot.as_mut().map(|s| &mut s.1).expect("just set")
        }
    };
    let r = ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(buffer)
    });
    if r.inner.lost_focus() && *buffer != entity.name {
        let name = buffer.clone();
        session.execute(Command::Rename { id, name });
    } else if !r.inner.has_focus() && *buffer != entity.name {
        *buffer = entity.name.clone();
    }

    // Parent.
    let parent_name = |p: Option<EntityId>| {
        p.and_then(|p| session.scene.get(p))
            .map_or("(none)".to_owned(), |e| e.name.clone())
    };
    let mut new_parent = None;
    ui.horizontal(|ui| {
        ui.label("Parent");
        egui::ComboBox::from_id_salt("parent")
            .selected_text(parent_name(entity.parent))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(entity.parent.is_none(), "(none)")
                    .clicked()
                {
                    new_parent = Some(None);
                }
                for e in &session.scene.entities {
                    if session.scene.is_ancestor_or_self(id, e.id) {
                        continue; // itself or a descendant: would be a cycle
                    }
                    if ui
                        .selectable_label(entity.parent == Some(e.id), &e.name)
                        .clicked()
                    {
                        new_parent = Some(Some(e.id));
                    }
                }
            });
        ui.checkbox(&mut state.keep_world_on_reparent, "keep world position");
    });
    if let Some(parent) = new_parent
        && parent != entity.parent
    {
        session.execute(Command::SetParent {
            id,
            parent,
            keep_world_transform: state.keep_world_on_reparent,
        });
    }

    ui.separator();
    transform_editor(ui, session, state, id);
    ui.separator();
    mesh_editor(ui, session, resources, id, entity.mesh.as_ref());
    // Gameplay components (the mesh editor may have changed the entity).
    let Some(entity) = session.scene.get(id).cloned() else {
        return;
    };
    ui.separator();
    visuals::animator_editor(ui, session, resources, &entity);
    ui.separator();
    visuals::light_editor(ui, session, &entity);
    ui.separator();
    visuals::shadow_editor(ui, session, &entity);
    ui.separator();
    gameplay::collider_editor(ui, session, &entity);
    ui.separator();
    let sounds = gameplay::sounds(session);
    gameplay::audio_editor(ui, session, &entity, &sounds);
    ui.separator();
    gameplay::behaviors_editor(ui, session, tools, &entity);
}

fn transform_editor(ui: &mut egui::Ui, session: &mut EditorSession, state: &UiState, id: EntityId) {
    let Some(t) = session.scene.get(id).map(|e| e.transform) else {
        return;
    };
    let mut translation = t.translation.to_array();
    let mut euler = quat_to_euler_deg(t.rotation).to_array();
    let mut scale = t.scale.to_array();
    let row = |ui: &mut egui::Ui, label: &str, v: &mut [f32; 3], speed: f64, step: Option<f32>| {
        let mut changed = false;
        let mut finished = false;
        ui.horizontal(|ui| {
            ui.label(label);
            for (c, axis) in v.iter_mut().zip(["x", "y", "z"]) {
                let r = ui.add(
                    egui::DragValue::new(c)
                        .speed(speed)
                        .prefix(format!("{axis} "))
                        .max_decimals(3),
                );
                if r.changed() {
                    changed = true;
                    if let Some(step) = step {
                        *c = snap(*c, step);
                    }
                }
                finished |= r.drag_stopped() || r.lost_focus();
            }
        });
        (changed, finished)
    };
    let step = |s: f32| state.snap_enabled.then_some(s);
    egui::Grid::new("transform").num_columns(1).show(ui, |ui| {
        let (c1, f1) = row(
            ui,
            "Position",
            &mut translation,
            0.01,
            step(state.snap_translate),
        );
        ui.end_row();
        let (c2, f2) = row(ui, "Rotation°", &mut euler, 0.5, step(state.snap_rotate));
        ui.end_row();
        let (c3, f3) = row(ui, "Scale", &mut scale, 0.01, step(state.snap_scale));
        ui.end_row();
        let key = format!("inspector-transform:{id}");
        if c1 {
            session.execute_merging(
                Command::SetTransform {
                    id,
                    translation: Some(Vec3::from(translation)),
                    rotation: None,
                    rotation_euler_deg: None,
                    scale: None,
                },
                Some(&key),
            );
        }
        if c2 {
            session.execute_merging(
                Command::SetTransform {
                    id,
                    translation: None,
                    rotation: None,
                    rotation_euler_deg: Some(Vec3::from(euler)),
                    scale: None,
                },
                Some(&key),
            );
        }
        if c3 {
            // Zero scale is rejected by validation; clamp while dragging.
            let s = Vec3::from(scale).map(|v| {
                if v.abs() < copperframe_scene::transform::MIN_SCALE {
                    copperframe_scene::transform::MIN_SCALE.copysign(v)
                } else {
                    v
                }
            });
            session.execute_merging(
                Command::SetTransform {
                    id,
                    translation: None,
                    rotation: None,
                    rotation_euler_deg: None,
                    scale: Some(s),
                },
                Some(&key),
            );
        }
        if f1 || f2 || f3 {
            session.end_merge();
        }
    });
}

fn assets_of(session: &EditorSession, kind: AssetKind) -> Vec<(AssetId, String)> {
    session
        .db
        .entries
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| (e.id, e.relative.clone()))
        .collect()
}

fn asset_label(session: &EditorSession, id: AssetId) -> String {
    session
        .db
        .get(id)
        .map_or_else(|| format!("MISSING {id}"), |e| e.relative.clone())
}

fn mesh_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    id: EntityId,
    mesh: Option<&MeshComponent>,
) {
    ui.label(egui::RichText::new("Mesh").strong());
    let Some(mesh) = mesh else {
        ui.horizontal_wrapped(|ui| {
            for shape in PrimitiveShape::ALL {
                if ui.button(format!("+ {}", shape.label())).clicked() {
                    session.execute(Command::SetMesh {
                        id,
                        mesh: Some(MeshComponent {
                            source: MeshSource::Primitive {
                                shape,
                                texture: None,
                            },
                            material: Default::default(),
                        }),
                    });
                }
            }
        });
        let models = assets_of(session, AssetKind::Gltf);
        if !models.is_empty() {
            egui::ComboBox::from_id_salt("add-model")
                .selected_text("+ Model…")
                .show_ui(ui, |ui| {
                    for (asset, path) in models {
                        if ui.selectable_label(false, path).clicked() {
                            session.execute(Command::SetMesh {
                                id,
                                mesh: Some(MeshComponent {
                                    source: MeshSource::Model { asset },
                                    material: Default::default(),
                                }),
                            });
                        }
                    }
                });
        }
        return;
    };
    let mut source = mesh.source.clone();
    match &mut source {
        MeshSource::Primitive { shape, texture } => {
            ui.horizontal(|ui| {
                ui.label("Shape");
                egui::ComboBox::from_id_salt("shape")
                    .selected_text(shape.label())
                    .show_ui(ui, |ui| {
                        for s in PrimitiveShape::ALL {
                            ui.selectable_value(shape, s, s.label());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Texture");
                let text = texture.map_or("(checker)".to_owned(), |t| asset_label(session, t));
                egui::ComboBox::from_id_salt("texture")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(texture, None, "(checker)");
                        for (asset, path) in assets_of(session, AssetKind::Texture) {
                            ui.selectable_value(texture, Some(asset), path);
                        }
                    });
            });
            if let Some(t) = texture {
                status_line(ui, resources.status(*t));
            }
        }
        MeshSource::Model { asset } => {
            ui.horizontal(|ui| {
                ui.label("Model");
                egui::ComboBox::from_id_salt("model")
                    .selected_text(asset_label(session, *asset))
                    .show_ui(ui, |ui| {
                        for (a, path) in assets_of(session, AssetKind::Gltf) {
                            ui.selectable_value(asset, a, path);
                        }
                    });
            });
            status_line(ui, resources.status(*asset));
        }
    }
    if source != mesh.source {
        session.execute(Command::SetMesh {
            id,
            mesh: Some(MeshComponent {
                source,
                material: mesh.material,
            }),
        });
    }

    // Material overrides: each field is either "from source" or a value.
    let mut m = mesh.material;
    let mut finished = false;
    ui.label(egui::RichText::new("Material overrides").strong());
    let mut tint_on = m.tint.is_some();
    ui.horizontal(|ui| {
        ui.checkbox(&mut tint_on, "Tint");
        if tint_on {
            let tint = m.tint.get_or_insert([1.0; 4]);
            for c in tint.iter_mut() {
                let r = ui.add(
                    egui::DragValue::new(c)
                        .speed(0.01)
                        .range(0.0..=4.0)
                        .max_decimals(2),
                );
                finished |= r.drag_stopped() || r.lost_focus();
            }
        } else {
            m.tint = None;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Alpha");
        let text = match m.alpha {
            None => "from source",
            Some(AlphaSetting::Opaque) => "opaque",
            Some(AlphaSetting::Cutout { .. }) => "cutout",
            Some(AlphaSetting::Blend) => "blend",
        };
        egui::ComboBox::from_id_salt("alpha")
            .selected_text(text)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut m.alpha, None, "from source");
                ui.selectable_value(&mut m.alpha, Some(AlphaSetting::Opaque), "opaque");
                if ui
                    .selectable_label(
                        matches!(m.alpha, Some(AlphaSetting::Cutout { .. })),
                        "cutout",
                    )
                    .clicked()
                {
                    m.alpha = Some(AlphaSetting::Cutout { threshold: 0.5 });
                }
                ui.selectable_value(&mut m.alpha, Some(AlphaSetting::Blend), "blend");
            });
        if let Some(AlphaSetting::Cutout { threshold }) = &mut m.alpha {
            let r = ui.add(egui::Slider::new(threshold, 0.0..=1.0));
            finished |= r.drag_stopped();
        }
    });
    let tri = |ui: &mut egui::Ui, label: &str, v: &mut Option<bool>| {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.selectable_value(v, None, "source");
            ui.selectable_value(v, Some(true), "on");
            ui.selectable_value(v, Some(false), "off");
        });
    };
    tri(ui, "Lit", &mut m.lit);
    tri(ui, "Flat shading", &mut m.flat_shading);
    tri(ui, "Affine UVs", &mut m.allow_affine);
    tri(ui, "Double-sided", &mut m.double_sided);
    if m != mesh.material {
        session.execute_merging(
            Command::SetMaterial { id, material: m },
            Some(&format!("material:{id}")),
        );
    }
    if finished {
        session.end_merge();
    }
    if ui.button("Remove mesh").clicked() {
        session.execute(Command::SetMesh { id, mesh: None });
    }
}

fn status_line(ui: &mut egui::Ui, status: Option<&AssetStatus>) {
    match status {
        None | Some(AssetStatus::Loaded { .. }) => {}
        Some(AssetStatus::Missing) => {
            ui.colored_label(
                level_color(LogLevel::Error),
                "missing from the project (fallback drawn)",
            );
        }
        Some(AssetStatus::Failed {
            error,
            kept_previous,
        }) => {
            let note = if *kept_previous {
                "showing last working import"
            } else {
                "fallback drawn"
            };
            ui.colored_label(
                level_color(LogLevel::Error),
                format!("import failed ({note}): {error}"),
            );
        }
    }
}

fn scene_settings(ui: &mut egui::Ui, session: &mut EditorSession, camera: &EditorCamera) {
    egui::CollapsingHeader::new("Scene settings")
        .default_open(session.selection.is_none())
        .show(ui, |ui| {
            let before = session.scene.settings.clone();
            let mut style = before.style.clone();
            let mut lighting = before.lighting;
            let mut finished = false;
            ui.horizontal(|ui| {
                ui.label(format!("Style: {}", style.preset().label()));
                if ui.button("PS1").clicked() {
                    style = RetroStyle::ps1();
                }
                if ui.button("PS2").clicked() {
                    style = RetroStyle::ps2();
                }
                if ui.button("Clean").clicked() {
                    style = RetroStyle::clean();
                }
            });
            ui.checkbox(&mut style.low_res.enabled, "Low resolution");
            ui.horizontal(|ui| {
                ui.checkbox(&mut style.vertex_snap.enabled, "Vertex snapping");
                let r = ui.add(
                    egui::Slider::new(&mut style.vertex_snap.grid, 0.25..=8.0).text("grid px"),
                );
                finished |= r.drag_stopped();
            });
            ui.checkbox(&mut style.affine_uv.enabled, "Affine UVs");
            ui.horizontal(|ui| {
                ui.checkbox(&mut style.color_reduction.enabled, "Colour reduction");
                let r =
                    ui.add(egui::Slider::new(&mut style.color_reduction.bits, 1..=8).text("bits"));
                finished |= r.drag_stopped();
            });
            ui.checkbox(&mut style.dither.enabled, "Dithering");
            ui.horizontal(|ui| {
                let linear = style.textures.filter == TextureFilter::Linear;
                let mut l = linear;
                ui.checkbox(&mut l, "Filtered textures");
                if l != linear {
                    style.textures.filter = if l {
                        TextureFilter::Linear
                    } else {
                        TextureFilter::Nearest
                    };
                }
                ui.checkbox(&mut style.textures.mipmaps, "Mipmaps");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut style.shading.per_pixel, "Per-pixel lighting");
                ui.checkbox(&mut style.shading.local_lights, "Point lights");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut style.shadows.enabled, "Blob shadows");
                let r = ui
                    .add(egui::Slider::new(&mut style.shadows.opacity, 0.0..=1.0).text("opacity"));
                finished |= r.drag_stopped();
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut style.fog.enabled, "Fog");
                ui.color_edit_button_srgb(&mut style.fog.color);
                let r1 = ui.add(
                    egui::DragValue::new(&mut style.fog.start)
                        .speed(0.1)
                        .prefix("start "),
                );
                let r2 = ui.add(
                    egui::DragValue::new(&mut style.fog.end)
                        .speed(0.1)
                        .prefix("end "),
                );
                finished |= r1.drag_stopped() || r2.drag_stopped();
            });
            ui.horizontal(|ui| {
                ui.label("Background");
                ui.color_edit_button_srgb(&mut style.atmosphere.background);
            });
            ui.horizontal(|ui| {
                ui.label("Light dir");
                for c in [
                    &mut lighting.direction.x,
                    &mut lighting.direction.y,
                    &mut lighting.direction.z,
                ] {
                    let r = ui.add(egui::DragValue::new(c).speed(0.01).max_decimals(2));
                    finished |= r.drag_stopped();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Light / ambient");
                // Edits set a grey light; the file may hold any RGB.
                let mut light = lighting.color.x;
                let mut ambient = lighting.ambient.x;
                let r1 = ui.add(
                    egui::DragValue::new(&mut light)
                        .speed(0.01)
                        .range(0.0..=4.0)
                        .max_decimals(2),
                );
                let r2 = ui.add(
                    egui::DragValue::new(&mut ambient)
                        .speed(0.01)
                        .range(0.0..=4.0)
                        .max_decimals(2),
                );
                finished |= r1.drag_stopped() || r2.drag_stopped();
                if r1.changed() {
                    lighting.color = Vec3::splat(light);
                }
                if r2.changed() {
                    lighting.ambient = Vec3::splat(ambient);
                }
            });
            let style_changed = style != before.style;
            let lighting_changed = lighting != before.lighting;
            if (style_changed || lighting_changed) && style.validate().is_ok() {
                session.execute_merging(
                    Command::SetSettings {
                        style: style_changed.then_some(style),
                        lighting: lighting_changed.then_some(lighting),
                        camera: None,
                    },
                    Some("scene-settings"),
                );
            }
            if finished {
                session.end_merge();
            }
            if ui.button("Use current view as start camera").clicked() {
                session.execute(Command::SetSettings {
                    style: None,
                    lighting: None,
                    camera: Some(camera.to_start()),
                });
            }
        });
}

fn bottom_panel(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &EditorCamera,
    resources: &SceneResources,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.bottom, BottomTab::Assets, "Assets");
        ui.selectable_value(&mut state.bottom, BottomTab::Console, "Console");
    });
    ui.separator();
    match state.bottom {
        BottomTab::Assets => {
            ui.horizontal(|ui| {
                if ui.button("Rescan assets folder").clicked() {
                    session.rescan_assets();
                    actions.push(Action::AssetsChanged);
                }
                ui.label(format!("{}", session.project.assets_dir().display()));
            });
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("assets").striped(true).show(ui, |ui| {
                    for entry in session.db.entries.clone() {
                        ui.label(&entry.relative);
                        ui.label(entry.kind.label());
                        match resources.status(entry.id) {
                            None => ui.label("not loaded"),
                            Some(AssetStatus::Loaded { warnings }) if warnings.is_empty() => {
                                ui.label("ok")
                            }
                            Some(AssetStatus::Loaded { warnings }) => ui
                                .colored_label(
                                    level_color(LogLevel::Warning),
                                    format!("{} warnings", warnings.len()),
                                )
                                .on_hover_text(warnings.join("\n")),
                            Some(AssetStatus::Failed { error, .. }) => ui
                                .colored_label(level_color(LogLevel::Error), "failed")
                                .on_hover_text(error),
                            Some(AssetStatus::Missing) => ui.label("missing"),
                        };
                        if entry.kind == AssetKind::Gltf && ui.button("Add to scene").clicked() {
                            session.add_model(entry.id, camera.spawn_point(4.0));
                        }
                        if entry.kind != AssetKind::Audio && ui.button("Reimport").clicked() {
                            actions.push(Action::Reimport(entry.id));
                        }
                        ui.end_row();
                    }
                });
            });
        }
        BottomTab::Console => {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &session.log {
                        ui.colored_label(level_color(line.level), &line.text);
                    }
                });
        }
    }
}

/// The 3D viewport: picking, camera controls, selection outline and the
/// translate gizmo. Returns the panel rectangle in points.
fn viewport(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &mut EditorCamera,
    resources: &SceneResources,
    dt: f32,
) -> Option<egui::Rect> {
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let panel = Rect {
        x: (rect.min.x * ppp).round().max(0.0) as u32,
        y: (rect.min.y * ppp).round().max(0.0) as u32,
        width: (rect.width() * ppp).round().max(0.0) as u32,
        height: (rect.height() * ppp).round().max(0.0) as u32,
    };
    let geometry = ViewportGeometry::new(panel, &session.scene.settings.style)?;
    let view_proj = camera.camera.view_projection(geometry.aspect());
    let to_px = |p: Pos2| Vec2::new(p.x * ppp, p.y * ppp);
    let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);

    // Camera: right mouse look + WASD/QE; wheel dolly.
    let looking = response.dragged_by(egui::PointerButton::Secondary);
    if looking {
        let d = response.drag_delta();
        let (motion, fast) = ui.input(|i| {
            let axis = |pos: Key, neg: Key| f32::from(i.key_down(pos)) - f32::from(i.key_down(neg));
            (
                Vec3::new(
                    axis(Key::D, Key::A),
                    axis(Key::E, Key::Q),
                    axis(Key::W, Key::S),
                ),
                i.modifiers.shift,
            )
        });
        camera.update(Vec2::new(d.x, d.y), motion, fast, dt);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            camera.camera.position += camera.camera.forward() * scroll * 0.01;
        }
    }

    let painter = ui.painter_at(rect);
    let world = session.scene.world_matrices();

    // Collider wireframes (green static, orange dynamic, blue kinematic,
    // yellow sensor).
    if state.show_colliders {
        for e in &session.scene.entities {
            let (Some(c), Some(&m)) = (&e.collider, world.get(&e.id)) else {
                continue;
            };
            let bounds = (c.shape == copperframe_scene::ColliderShape::Mesh)
                .then(|| resources.local_bounds(&session.scene, e.id))
                .flatten();
            let width = if session.selection == Some(e.id) {
                1.5
            } else {
                1.0
            };
            let stroke = Stroke::new(width, gameplay::collider_color(c).gamma_multiply(0.8));
            for (a, b) in gameplay::collider_lines(c, m, bounds) {
                if let (Some(pa), Some(pb)) = (
                    geometry.project(view_proj, a),
                    geometry.project(view_proj, b),
                ) {
                    painter.line_segment([to_pos(pa), to_pos(pb)], stroke);
                }
            }
        }
    }

    // Selection outline (local bounds box) and gizmo.
    let mut gizmo_axes: Vec<(usize, Vec2, Vec2)> = Vec::new();
    if let Some(id) = session.selection
        && let Some(&m) = world.get(&id)
    {
        if let Some((lo, hi)) = resources.local_bounds(&session.scene, id) {
            let corners: Vec<Option<Vec2>> = (0..8)
                .map(|i| {
                    let c = Vec3::new(
                        if i & 1 == 0 { lo.x } else { hi.x },
                        if i & 2 == 0 { lo.y } else { hi.y },
                        if i & 4 == 0 { lo.z } else { hi.z },
                    );
                    geometry.project(view_proj, m.transform_point3(c))
                })
                .collect();
            let stroke = Stroke::new(1.5, Color32::from_rgb(255, 200, 60));
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
                if let (Some(pa), Some(pb)) = (corners[a], corners[b]) {
                    painter.line_segment([to_pos(pa), to_pos(pb)], stroke);
                }
            }
        }
        let origin = m.w_axis.truncate();
        if let Some(o) = geometry.project(view_proj, origin) {
            // Arrow length ~ 80 points regardless of distance.
            let dist = origin.distance(camera.camera.position).max(0.1);
            let world_len = dist * 0.15;
            for (i, axis) in AXES.iter().enumerate() {
                if let Some(tip) = geometry.project(view_proj, origin + *axis * world_len) {
                    gizmo_axes.push((i, o, tip));
                }
            }
        }
    }
    let colors = [
        Color32::from_rgb(230, 70, 70),
        Color32::from_rgb(90, 210, 90),
        Color32::from_rgb(80, 130, 240),
    ];
    let axis_at = |p: Pos2| {
        let p = to_px(p);
        gizmo_axes
            .iter()
            .map(|&(i, a, b)| (i, dist_to_segment(p, a, b)))
            .filter(|&(_, d)| d < 8.0 * ppp)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let hovered_axis = response.hover_pos().and_then(axis_at);
    for &(i, a, b) in &gizmo_axes {
        let active = state.gizmo.map(|g| g.axis) == Some(i) || hovered_axis == Some(i);
        let width = if active { 4.0 } else { 2.5 };
        painter.line_segment([to_pos(a), to_pos(b)], Stroke::new(width, colors[i]));
        painter.circle_filled(to_pos(b), if active { 5.0 } else { 4.0 }, colors[i]);
    }

    // Gizmo drag.
    // Hit-test where the button went down, not where the pointer is once
    // the drag threshold is passed.
    let press_axis = ui.input(|i| i.pointer.press_origin()).and_then(axis_at);
    if response.drag_started_by(egui::PointerButton::Primary)
        && let (Some(axis), Some(id)) = (press_axis, session.selection)
        && let Some(m) = world.get(&id)
    {
        state.gizmo = Some(GizmoDrag {
            entity: id,
            axis,
            start_world: m.w_axis.truncate(),
            pixels: Vec2::ZERO,
        });
    }
    if let Some(g) = &mut state.gizmo {
        if response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            g.pixels += Vec2::new(d.x, d.y) * ppp;
            let g = *g;
            if let Some(dist) =
                axis_drag_distance(&geometry, view_proj, g.start_world, AXES[g.axis], g.pixels)
            {
                let dist = if state.snap_enabled {
                    snap(dist, state.snap_translate)
                } else {
                    dist
                };
                let target = g.start_world + AXES[g.axis] * dist;
                let parent_world = session
                    .scene
                    .get(g.entity)
                    .and_then(|e| e.parent)
                    .and_then(|p| world.get(&p).copied())
                    .unwrap_or(Mat4::IDENTITY);
                let local = parent_world.inverse().transform_point3(target);
                session.execute_merging(
                    Command::SetTransform {
                        id: g.entity,
                        translation: Some(local),
                        rotation: None,
                        rotation_euler_deg: None,
                        scale: None,
                    },
                    Some(&format!("gizmo:{}", g.entity)),
                );
            }
        }
        if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            state.gizmo = None;
            session.end_merge();
        }
    } else if response.clicked_by(egui::PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        // Picking: window pixels -> ray -> nearest mesh triangle.
        session.selection = geometry
            .ray(view_proj, to_px(p))
            .and_then(|(o, d)| resources.pick(&session.scene, &world, o, d))
            .map(|(id, _)| id);
    }

    if looking {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    Some(rect)
}

fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn modals(
    ctx: &egui::Context,
    session: &mut EditorSession,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    if state.confirm_close {
        egui::Modal::new(egui::Id::unique("copperframe-confirm-close")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label(format!(
                "{} has unsaved changes.",
                session.project.relative(&session.scene_path)
            ));
            ui.horizontal(|ui| {
                if ui.button("Save and quit").clicked() {
                    save(session, state, false);
                    if !session.is_dirty() {
                        actions.push(Action::Quit);
                    }
                    state.confirm_close = false;
                }
                if ui.button("Quit without saving").clicked() {
                    actions.push(Action::Quit);
                    state.confirm_close = false;
                }
                if ui.button("Cancel").clicked() {
                    state.confirm_close = false;
                }
            });
        });
    }
    if let Some(disk) = state.conflict {
        egui::Modal::new(egui::Id::unique("copperframe-save-conflict")).show(ctx, |ui| {
            ui.heading("Scene changed on disk");
            ui.label(format!(
                "{} is at revision {disk} on disk, but the editor loaded revision {}.\n\
                 Another tool changed it while the editor was open.",
                session.project.relative(&session.scene_path),
                session.scene.revision
            ));
            ui.horizontal(|ui| {
                if ui.button("Overwrite with my version").clicked() {
                    save(session, state, true);
                }
                if ui.button("Reload from disk (discard my changes)").clicked() {
                    if let Err(e) = session.reload() {
                        session.error(e.to_string());
                    }
                    state.conflict = None;
                }
                if ui.button("Cancel").clicked() {
                    state.conflict = None;
                }
            });
        });
    }
}
