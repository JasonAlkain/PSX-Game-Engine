//! Menus, the hierarchy, inspector, scene settings and console panels, and
//! dialogs. The panels sit in the [`crate::dock`] layout.
//!
//! Panels never mutate the scene directly; they build [`Command`]s and run
//! them through the [`EditorSession`], so every change is validated and
//! undoable. Continuous edits (drags, typing) merge into one undo step per
//! gesture.

use copperframe_assets::AssetKind;
use copperframe_core::{AssetId, EntityId};
use copperframe_render::Rect;
use copperframe_runtime::scene_gpu::{AssetStatus, SceneResources};
use copperframe_scene::{
    AlphaSetting, Command, LookSource, MeshComponent, MeshSource, PrimitiveShape, quat_to_euler_deg,
};
use egui::{Color32, Key, KeyboardShortcut, Modifiers};
use glam::Vec3;

use std::path::PathBuf;

use crate::assets_panel::AssetsPanel;
use crate::assets_panel::PanelRequest;
use crate::audio_tools::EditorAudio;
use crate::components;
use crate::dock::{Dock, Pane, PaneContents};
use crate::gameplay::{GameTools, PlayState};
use crate::gizmo::{Tool, ToolPrefs};
use crate::hierarchy::HierarchyPanel;
use crate::hub::{self, RecentProject};
use crate::project_settings::ProjectSettingsPanel;
use crate::reporting::Reporting;
use crate::session::{
    EditorSession, LogLevel, SCENE_SUFFIX, SaveOutcome, SceneTemplate, scene_stem,
};
use crate::viewport::{EditorCamera, snap};
use crate::viewport_panel::{self, GizmoDrag};
use crate::{look_editor, theme, toolbar};

/// Requests the app must carry out (they need the GPU, the event loop or
/// another project).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Reimport(AssetId),
    /// Asset database was rescanned; retry assets that failed to load.
    AssetsChanged,
    /// Switch to another scene of this project.
    OpenScene(PathBuf),
    /// Create a scene file from a template and switch to it.
    NewScene {
        path: PathBuf,
        name: String,
        template: SceneTemplate,
    },
    /// Close this project and open another.
    OpenProject(PathBuf),
    /// Close this project and show the project hub.
    CloseProject,
    /// Close this project and show the hub's new project form.
    NewProject,
    Quit,
}

impl Action {
    /// Whether the action throws away the open scene's unsaved edits (so
    /// the user is asked first).
    fn leaves_scene(&self) -> bool {
        matches!(
            self,
            Action::OpenScene(_)
                | Action::NewScene { .. }
                | Action::OpenProject(_)
                | Action::CloseProject
                | Action::NewProject
                | Action::Quit
        )
    }

    /// Whether the action closes the project (so unsaved project settings
    /// would be lost).
    fn leaves_project(&self) -> bool {
        self.leaves_scene() && !matches!(self, Action::OpenScene(_) | Action::NewScene { .. })
    }

    fn verb(&self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::OpenScene(_) => "open the other scene",
            Action::NewScene { .. } => "create the new scene",
            Action::OpenProject(_) | Action::NewProject => "switch project",
            Action::CloseProject => "close the project",
            Action::Reimport(_) | Action::AssetsChanged => "continue",
        }
    }
}

/// Frame statistics shown in the status bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub fps: f32,
    pub draws: usize,
    pub triangles: u64,
    /// False while the editor waits for input instead of redrawing.
    pub continuous: bool,
}

/// UI state that is not part of the scene.
pub struct UiState {
    /// Toolbar settings (saved with the layout).
    pub prefs: ToolPrefs,
    /// The panel layout.
    pub dock: Dock,
    pub assets: AssetsPanel,
    pub project_settings: ProjectSettingsPanel,
    pub hierarchy: HierarchyPanel,
    /// Sound previews and the mixer's meters.
    pub audio: EditorAudio,
    keep_world_on_reparent: bool,
    name_buffer: Option<(EntityId, String)>,
    pub(crate) drag: Option<GizmoDrag>,
    /// The viewport camera is being flown (right mouse held).
    pub(crate) camera_active: bool,
    /// An action waiting for "save or discard?" (unsaved changes).
    confirm: Option<Action>,
    conflict: Option<u64>,
    /// Another program changed the open scene file (its revision).
    pub disk_changed: Option<u64>,
    /// File > New Scene or Save Scene As, while open.
    scene_dialog: Option<SceneDialog>,
    pub stats: Stats,
    /// Crash prompt and bug report dialog.
    pub reporting: Reporting,
    /// The Grid and snapping window is open.
    pub snap_settings_open: bool,
    /// A panel to open once the dock is back (panels can't open panels
    /// while they draw).
    open_pane: Option<Pane>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            prefs: ToolPrefs::default(),
            dock: Dock::default(),
            assets: AssetsPanel::default(),
            project_settings: ProjectSettingsPanel::default(),
            hierarchy: HierarchyPanel::default(),
            audio: EditorAudio::default(),
            keep_world_on_reparent: true,
            name_buffer: None,
            drag: None,
            camera_active: false,
            confirm: None,
            conflict: None,
            disk_changed: None,
            scene_dialog: None,
            stats: Stats::default(),
            reporting: Reporting::default(),
            snap_settings_open: false,
            open_pane: None,
        }
    }
}

impl UiState {
    /// Something in the UI moves on its own and needs a frame every
    /// refresh: flying the camera (keys held without mouse events) or a
    /// gizmo drag.
    pub fn is_animating(&self) -> bool {
        self.camera_active || self.drag.is_some()
    }

    /// Asks before closing when there are unsaved changes. Returns true if
    /// the app may quit now.
    pub fn request_close(&mut self, session: &EditorSession) -> bool {
        if self.loses_edits(session, &Action::Quit) {
            self.confirm = Some(Action::Quit);
            false
        } else {
            true
        }
    }

    /// Whether `action` would throw away unsaved scene edits or project
    /// settings.
    fn loses_edits(&self, session: &EditorSession, action: &Action) -> bool {
        (action.leaves_scene() && session.is_dirty())
            || (action.leaves_project() && self.project_settings.is_dirty())
    }

    /// Queues `action`, asking first if it would discard unsaved edits.
    pub fn request(&mut self, session: &EditorSession, action: Action, actions: &mut Vec<Action>) {
        if self.loses_edits(session, &action) {
            self.confirm = Some(action);
        } else {
            actions.push(action);
        }
    }
}

/// Which scene file dialog is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SceneDialogKind {
    New,
    SaveAs,
}

/// The New Scene / Save Scene As dialog: a name, a folder under `scenes/`
/// and (for a new scene) a template.
#[derive(Debug, Clone, PartialEq)]
struct SceneDialog {
    kind: SceneDialogKind,
    name: String,
    folder: String,
    template: SceneTemplate,
    error: Option<String>,
}

impl SceneDialog {
    fn new(kind: SceneDialogKind, session: &EditorSession, folder: Option<String>) -> Self {
        let folder = folder.unwrap_or_else(|| {
            // Next to the open scene.
            session
                .scene_path
                .parent()
                .map(|p| session.project.relative(p))
                .filter(|f| f.starts_with(copperframe_assets::project::SCENES_DIR))
                .unwrap_or_else(|| copperframe_assets::project::SCENES_DIR.to_owned())
        });
        let name = match kind {
            SceneDialogKind::New => "New scene".to_owned(),
            SceneDialogKind::SaveAs => format!("{} copy", session.scene.name),
        };
        Self {
            kind,
            name,
            folder,
            template: SceneTemplate::Empty,
            error: None,
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
    /// Recently opened projects, for File > Recent Projects.
    pub recent: &'a [RecentProject],
    pub dt: f32,
}

/// The panels' contents, for the dock to place.
struct Panes<'a> {
    session: &'a mut EditorSession,
    tools: &'a mut GameTools,
    state: &'a mut UiState,
    camera: &'a mut EditorCamera,
    resources: &'a SceneResources,
    dt: f32,
    actions: &'a mut Vec<Action>,
    /// Set if the viewport panel is showing this frame.
    viewport: &'a mut Option<egui::Rect>,
}

impl PaneContents for Panes<'_> {
    fn pane(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Viewport => {
                *self.viewport = viewport_panel::show(
                    ui,
                    self.session,
                    self.state,
                    self.camera,
                    self.resources,
                    self.dt,
                );
            }
            Pane::Hierarchy => {
                let keep_world = self.state.keep_world_on_reparent;
                self.state
                    .hierarchy
                    .show(ui, self.session, self.camera, keep_world);
            }
            Pane::Inspector => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        inspector(ui, self.session, self.state, self.resources, self.tools);
                    });
            }
            Pane::SceneSettings => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        scene_settings(ui, self.session, self.state, self.camera)
                    });
            }
            Pane::Assets => {
                let mixer = self.state.project_settings.audio_settings(self.session);
                self.state.assets.show(
                    ui,
                    self.session,
                    self.camera,
                    self.resources,
                    self.actions,
                    &mut self.state.audio,
                    mixer,
                );
            }
            Pane::AudioMixer => {
                self.state.project_settings.mixer(
                    ui,
                    self.session,
                    self.tools,
                    &mut self.state.audio,
                );
            }
            Pane::Problems => crate::scripting::problems_panel(ui, self.session, self.tools),
            Pane::Console => console(ui, self.session),
            Pane::ProjectSettings => {
                self.state
                    .project_settings
                    .show(ui, self.session, self.tools);
            }
        }
    }
}

fn console(ui: &mut egui::Ui, session: &EditorSession) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &session.log {
                ui.colored_label(level_color(line.level), &line.text);
            }
        });
}

pub fn draw(ui: &mut egui::Ui, cx: UiContext<'_>) -> UiOutput {
    let UiContext {
        session,
        tools,
        state,
        camera,
        resources,
        recent,
        dt,
    } = cx;
    let mut actions = Vec::new();
    let mixer = state.project_settings.audio_settings(session);
    state.audio.sync(ui.ctx(), mixer);
    shortcuts(ui.ctx(), session, state, tools, camera, resources);

    egui::Panel::top("menu").show(ui, |ui| {
        menu_bar(ui, session, state, camera, tools, recent, &mut actions);
    });
    egui::Panel::top("toolbar")
        .frame(
            egui::Frame::new()
                .fill(theme::BAR)
                .inner_margin(egui::Margin::symmetric(6, 3)),
        )
        .show(ui, |ui| {
            toolbar::show(ui, session, state, camera, resources, tools);
        });
    egui::Panel::bottom("status")
        .resizable(false)
        .show(ui, |ui| {
            status_bar(ui, session, state, tools.problems.len())
        });
    let ppp = ui.ctx().pixels_per_point();
    let mut viewport = None;
    egui::CentralPanel::no_frame().show(ui, |ui| {
        // The dock is taken out so the panels can borrow the rest of the state.
        let mut dock = state.dock.take();
        if dock.any_open() {
            dock.show(
                ui,
                &mut Panes {
                    session,
                    tools,
                    state,
                    camera,
                    resources,
                    dt,
                    actions: &mut actions,
                    viewport: &mut viewport,
                },
            );
        } else {
            ui.painter().rect_filled(ui.max_rect(), 0.0, theme::PANEL);
            ui.centered_and_justified(|ui| {
                ui.label("Every panel is closed. Open one from the Window menu.");
            });
        }
        state.dock = dock;
    });
    if std::mem::take(&mut tools.problems.arrived)
        | std::mem::take(&mut state.project_settings.open_problems)
    {
        state.open_pane = Some(Pane::Problems);
    }
    if std::mem::take(&mut state.project_settings.open_mixer) {
        state.open_pane = Some(Pane::AudioMixer);
    }
    if let Some(pane) = state.open_pane.take() {
        state.dock.set_open(pane, true);
    }
    for request in std::mem::take(&mut state.assets.requests) {
        match request {
            PanelRequest::OpenScene(path) => {
                state.request(session, Action::OpenScene(path), &mut actions);
            }
            PanelRequest::NewScene(folder) => {
                state.scene_dialog = Some(SceneDialog::new(
                    SceneDialogKind::New,
                    session,
                    Some(folder),
                ));
            }
            PanelRequest::NewScript { folder, csharp } => {
                let mut dialog = crate::scripting::NewScript::new(&session.project, None);
                dialog.csharp = csharp;
                dialog.folder = folder;
                tools.new_script = Some(dialog);
            }
        }
    }
    let viewport = viewport.map(|r| viewport_panel::to_pixels(r, ppp));

    modals(ui.ctx(), session, state, &mut actions);
    build_dialogs(ui.ctx(), session, tools);
    crate::scripting::new_script_dialog(ui.ctx(), session, tools);
    crate::snap_settings::show(ui.ctx(), session, state);
    state.reporting.show(ui.ctx(), session);
    UiOutput { viewport, actions }
}

fn shortcuts(
    ctx: &egui::Context,
    session: &mut EditorSession,
    state: &mut UiState,
    tools: &mut GameTools,
    camera: &mut EditorCamera,
    resources: &SceneResources,
) {
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
        if tools.is_playing() {
            tools.stop(session);
        } else {
            tools.play(session);
        }
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F6)) {
        tools.toggle_pause(session);
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F7)) {
        tools.step(session);
    }
    let cmd = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
    let cmd_shift = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
    // Shift first: Ctrl+S would also match Ctrl+Shift+S.
    if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::S))) {
        state.scene_dialog = Some(SceneDialog::new(SceneDialogKind::SaveAs, session, None));
    } else if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
        save(session, state, false);
    }
    if state.scene_dialog.is_none() && ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::N))) {
        state.scene_dialog = Some(SceneDialog::new(SceneDialogKind::New, session, None));
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
    // Q, W and E also fly the camera while the right mouse button is held.
    let flying = state.camera_active || ctx.input(|i| i.pointer.secondary_down());
    if !flying {
        for (key, tool) in [
            (Key::Q, Tool::Move),
            (Key::W, Tool::Rotate),
            (Key::E, Tool::Scale),
        ] {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                state.prefs.tool = tool;
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) {
            viewport_panel::frame_selection(session, resources, camera);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::OpenBracket)) {
            state.prefs.step_move(true);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::CloseBracket)) {
            state.prefs.step_move(false);
        }
    }
}

/// Window: every panel with a tick for whether it is open, and a way back to
/// the standard arrangement.
fn window_menu(ui: &mut egui::Ui, state: &mut UiState) {
    for pane in Pane::ALL {
        let mut open = state.dock.is_open(pane);
        if ui.checkbox(&mut open, pane.title()).changed() {
            state.dock.set_open(pane, open);
        }
    }
    ui.separator();
    if ui
        .button("Reset layout")
        .on_hover_text("Put every panel back in its standard place")
        .clicked()
    {
        state.dock.reset();
        ui.close();
    }
}

/// Saves unsaved edits before a job that reads the files on disk. Returns
/// false (and logs why) if the scene could not be saved.
fn save_for(session: &mut EditorSession, state: &mut UiState, what: &str) -> bool {
    if session.is_dirty() {
        save(session, state, false);
    }
    if session.is_dirty() {
        session.error(format!("{what} cancelled: the scene could not be saved"));
        return false;
    }
    true
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
    recent: &[RecentProject],
    actions: &mut Vec<Action>,
) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            file_menu(ui, session, state, recent, actions);
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
            if let Some(state) = tools.play_state() {
                if ui.button("Stop   F5").clicked() {
                    tools.stop(session);
                }
                let live = !matches!(state, PlayState::Starting | PlayState::Stopping);
                let paused = matches!(state, PlayState::Paused { .. } | PlayState::Stepping);
                let label = if paused { "Resume   F6" } else { "Pause   F6" };
                if ui.add_enabled(live, egui::Button::new(label)).clicked() {
                    tools.toggle_pause(session);
                }
                if ui.add_enabled(live, egui::Button::new("Step one tick   F7")).clicked() {
                    tools.step(session);
                }
            } else if ui.button("Play   F5").clicked() {
                tools.play(session);
            }
            if ui.button("Reload behaviour kinds").clicked() {
                tools.refresh_schema(session);
            }
            ui.separator();
            if ui
                .add_enabled(
                    crate::scripting::scripting_on(&session.project),
                    egui::Button::new("New script…"),
                )
                .on_disabled_hover_text("Scripting is off: Project settings > Scripting")
                .clicked()
            {
                tools.new_script = Some(crate::scripting::NewScript::new(
                    &session.project,
                    session.selection,
                ));
            }
            let n = tools.problems.len();
            let label = if n == 0 {
                "Problems".to_owned()
            } else {
                format!("Problems ({n})")
            };
            if ui.button(label).clicked() {
                state.dock.set_open(Pane::Problems, true);
            }
        });
        ui.menu_button("Build", |ui| build_menu(ui, session, state, tools));
        ui.menu_button("Window", |ui| window_menu(ui, state));
        ui.menu_button("Help", |ui| {
            ui.label(
                "Viewport: left click selects. Q moves, W rotates, E scales: drag the handles.\n\
                 F frames the selection. Hold V to drag a vertex onto another model's vertex.\n\
                 Hold the right mouse button to look; WASD/QE fly while held, Shift is faster;\n\
                 the wheel moves forward/back.\n\
                 Panels: drag a tab to move it, drag a border to resize, Window reopens a closed one.\n\
                 Ctrl+S save, Ctrl+Z undo, Ctrl+Y / Ctrl+Shift+Z redo, Ctrl+D duplicate, Del delete.\n\
                 F5 plays the scene in a separate game window (unsaved edits included) and stops it;\n\
                 F6 pauses and resumes it, F7 advances it by one tick.\n\
                 New and changed files in assets/ are imported automatically.",
            );
            ui.separator();
            if ui
                .button("Report a bug…")
                .on_hover_text("Opens an email draft with system details in your mail program")
                .clicked()
            {
                state.reporting.open_bug_report();
                ui.close();
            }
        });
    });
}

fn file_menu(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    recent: &[RecentProject],
    actions: &mut Vec<Action>,
) {
    if ui.button("New Project…").clicked() {
        state.request(session, Action::NewProject, actions);
    }
    if ui.button("Open Project…").clicked()
        && let Some(dir) =
            hub::pick_folder("Open a Copperframe project", session.project.root.parent())
    {
        state.request(session, Action::OpenProject(dir), actions);
    }
    let current = dunce::canonicalize(&session.project.root).ok();
    let others: Vec<&RecentProject> = recent
        .iter()
        .filter(|p| Some(&p.path) != current.as_ref())
        .take(10)
        .collect();
    ui.add_enabled_ui(!others.is_empty(), |ui| {
        ui.menu_button("Recent Projects", |ui| {
            for p in others {
                if ui
                    .button(&p.name)
                    .on_hover_text(p.path.display().to_string())
                    .clicked()
                {
                    state.request(session, Action::OpenProject(p.path.clone()), actions);
                }
            }
        });
    });
    if ui
        .button("Project Settings…")
        .on_hover_text("Name, start scene, window, look, keys and export options")
        .clicked()
    {
        state.dock.set_open(Pane::ProjectSettings, true);
    }
    if ui
        .button("Close Project")
        .on_hover_text("Back to the project hub")
        .clicked()
    {
        state.request(session, Action::CloseProject, actions);
    }
    ui.separator();
    if ui.button("New Scene…   Ctrl+N").clicked() {
        state.scene_dialog = Some(SceneDialog::new(SceneDialogKind::New, session, None));
    }
    let scenes = session.scene_files();
    ui.menu_button("Open Scene", |ui| {
        for path in scenes {
            let rel = session.project.relative(&path);
            let current = path == session.scene_path;
            let startup = path == session.project.startup_scene_path();
            let label = if startup {
                format!("{rel}  (start)")
            } else {
                rel
            };
            if ui.add_enabled(!current, egui::Button::new(label)).clicked() {
                state.request(session, Action::OpenScene(path), actions);
            }
        }
        ui.separator();
        if ui.button("Browse…").clicked()
            && let Some(path) = pick_scene(session)
        {
            state.request(session, Action::OpenScene(path), actions);
        }
    });
    let recent = session.recent_scene_paths();
    ui.add_enabled_ui(!recent.is_empty(), |ui| {
        ui.menu_button("Recent Scenes", |ui| {
            for path in recent {
                if ui.button(session.project.relative(&path)).clicked() {
                    state.request(session, Action::OpenScene(path), actions);
                }
            }
        });
    });
    if ui
        .add_enabled(
            !session.is_startup_scene(),
            egui::Button::new("Set as Start Scene"),
        )
        .on_hover_text("The exported game starts with this scene")
        .clicked()
    {
        session.set_startup_scene();
    }
    ui.separator();
    if ui.button("Save Scene   Ctrl+S").clicked() {
        save(session, state, false);
    }
    if ui.button("Save Scene As…   Ctrl+Shift+S").clicked() {
        state.scene_dialog = Some(SceneDialog::new(SceneDialogKind::SaveAs, session, None));
    }
    if ui.button("Reload from disk (discard changes)").clicked()
        && let Err(e) = session.reload()
    {
        session.error(e.to_string());
    }
    ui.separator();
    if ui.button("Quit").clicked() {
        state.request(session, Action::Quit, actions);
    }
}

fn build_menu(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    tools: &mut GameTools,
) {
    let idle = !tools.is_busy();
    if ui
        .add_enabled(idle, egui::Button::new("Validate Project"))
        .on_hover_text("Checks the project file, every asset and every scene (saves first)")
        .clicked()
        && save_for(session, state, "validation")
    {
        tools.validate(session);
    }
    ui.separator();
    ui.label("Export folder");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut tools.export_dir).desired_width(260.0));
        if ui.button("Browse…").clicked()
            && let Some(dir) = hub::pick_folder(
                "Export the game into this folder",
                std::path::Path::new(tools.export_dir.trim()).parent(),
            )
        {
            tools.export_dir = dir.display().to_string();
        }
    });
    if ui
        .add_enabled(idle, egui::Button::new("Export Game"))
        .on_hover_text(
            "Saves, then writes a standalone copy of the game (release build, project data, \
             licence notices) into the export folder, replacing an earlier export there, and \
             checks that it starts.",
        )
        .clicked()
        && save_for(session, state, "export")
    {
        tools.export(session);
    }
    ui.separator();
    ui.menu_button("Run Test Script", |ui| {
        let scripts = GameTools::test_scripts(session);
        if scripts.is_empty() {
            ui.label(egui::RichText::new("No *.script.json files in this project").weak());
        }
        for script in scripts {
            let rel = session.project.relative(&script);
            if ui.add_enabled(idle, egui::Button::new(rel)).clicked()
                && save_for(session, state, "the test")
            {
                tools.run_test(session, Some(script));
            }
        }
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new("Choose a script file…"))
            .clicked()
            && let Some(script) = hub::pick_file(
                "Choose a test script",
                Some(&session.project.root),
                "Test script",
                &["json"],
            )
            && save_for(session, state, "the test")
        {
            tools.run_test(session, Some(script));
        }
        if ui
            .add_enabled(idle, egui::Button::new("Idle run (no script)"))
            .on_hover_text("Plays the scene headless for 5 seconds and reports errors")
            .clicked()
            && save_for(session, state, "the test")
        {
            tools.run_test(session, None);
        }
    });
    ui.menu_button("Advanced", |ui| {
        if ui
            .add_enabled(idle, egui::Button::new("Benchmark Scene"))
            .on_hover_text(
                "Release build, then 300 frames offscreen at 1280x720 in the authored, PS1 and \
                 PS2 styles. Needs a graphics adapter.",
            )
            .clicked()
            && save_for(session, state, "the benchmark")
        {
            tools.bench(session);
        }
    });
    if tools.result.is_some() {
        ui.separator();
        ui.label(egui::RichText::new("The last result is open in its own window").weak());
    }
}

/// The Build results window and the "Rust is not installed" prompt.
fn build_dialogs(ctx: &egui::Context, session: &mut EditorSession, tools: &mut GameTools) {
    let mut close = false;
    if let Some(result) = &tools.result {
        let mut open = true;
        egui::Window::new(result.job.title())
            .id(egui::Id::unique("copperframe-build-result"))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .open(&mut open)
            .default_width(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let color = if result.passed {
                    Color32::from_rgb(120, 210, 120)
                } else {
                    level_color(LogLevel::Error)
                };
                ui.colored_label(color, &result.headline);
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&result.details).monospace())
                                .wrap(),
                        );
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(format!("{}\n\n{}", result.headline, result.details));
                    }
                    if let Some(folder) = &result.folder
                        && ui.button("Open folder").clicked()
                        && let Err(e) =
                            copperframe_core::report::open_external(&folder.display().to_string())
                    {
                        session.error(format!("could not open the folder: {e}"));
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        close |= !open;
    }
    if close {
        tools.result = None;
    }
    if tools.rust_prompt {
        egui::Modal::new(egui::Id::unique("copperframe-rust-missing")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Rust is needed for this project");
            ui.label(
                "This project has Rust game code (the game/ folder). Play, tests and export \
                 compile it, which needs Rust.",
            );
            ui.add_space(4.0);
            hub::rust_missing_note(ui);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Check again").clicked() {
                    tools.recheck_rust(session);
                }
                if ui.button("Close").clicked() {
                    tools.rust_prompt = false;
                }
            });
        });
    }
}

fn status_bar(ui: &mut egui::Ui, session: &EditorSession, state: &mut UiState, problems: usize) {
    ui.horizontal(|ui| {
        if problems > 0 {
            let text = egui::RichText::new(format!("{problems} script problem(s)"))
                .color(level_color(LogLevel::Warning));
            if ui
                .add(egui::Button::new(text).frame(false))
                .on_hover_text("Show the Problems panel")
                .clicked()
            {
                state.open_pane = Some(Pane::Problems);
            }
            ui.separator();
        }
        let dirty = if session.is_dirty() { " * unsaved" } else { "" };
        ui.label(format!(
            "{} — {} (revision {}){dirty}",
            session.project.file.name,
            session.project.relative(&session.scene_path),
            session.scene.revision
        ));
        ui.separator();
        let s = state.stats;
        let rate = if s.continuous {
            format!("{:.0} fps", s.fps)
        } else {
            "idle".to_string()
        };
        ui.label(format!(
            "{rate} · {} draws · {} triangles · {} entities",
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

pub(crate) fn level_color(level: LogLevel) -> Color32 {
    match level {
        LogLevel::Info => Color32::GRAY,
        LogLevel::Warning => Color32::from_rgb(230, 190, 60),
        LogLevel::Error => Color32::from_rgb(240, 90, 80),
    }
}

fn inspector(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    resources: &SceneResources,
    tools: &mut GameTools,
) {
    let Some(id) = session.selection else {
        ui.label("Nothing selected. Scene-wide options are in Scene settings.");
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
    components::components(ui, session, resources, tools, id);
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
    let step = |s: f32| state.prefs.grid_snap.then_some(s);
    egui::Grid::new("transform").num_columns(1).show(ui, |ui| {
        let (c1, f1) = row(
            ui,
            "Position",
            &mut translation,
            0.01,
            step(state.prefs.snap_translate),
        );
        ui.end_row();
        let (c2, f2) = row(
            ui,
            "Rotation°",
            &mut euler,
            0.5,
            step(state.prefs.snap_rotate),
        );
        ui.end_row();
        let (c3, f3) = row(ui, "Scale", &mut scale, 0.01, step(state.prefs.snap_scale));
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

pub(crate) fn assets_of(session: &EditorSession, kind: AssetKind) -> Vec<(AssetId, String)> {
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

pub(crate) fn mesh_editor(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    id: EntityId,
    mesh: Option<&MeshComponent>,
) {
    let Some(mesh) = mesh else {
        return;
    };
    let mut source = mesh.source.clone();
    let mut tile_finished = false;
    match &mut source {
        MeshSource::Primitive {
            shape,
            texture,
            tile_size,
        } => {
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
            if shape.tiles() {
                ui.horizontal(|ui| {
                    let mut on = tile_size.is_some();
                    ui.checkbox(&mut on, "Tile texture").on_hover_text(
                        "Repeat the texture by the entity's size in the world, so scaling \
                         it does not stretch the texture. Off covers each face once.",
                    );
                    *tile_size =
                        on.then(|| tile_size.unwrap_or(copperframe_scene::DEFAULT_TILE_SIZE));
                    if let Some(size) = tile_size {
                        let r = ui.add(
                            egui::DragValue::new(size)
                                .speed(0.05)
                                .range(
                                    copperframe_scene::MIN_TILE_SIZE
                                        ..=copperframe_scene::MAX_TILE_SIZE,
                                )
                                .suffix(" m per repeat"),
                        );
                        tile_finished = r.drag_stopped() || r.lost_focus();
                    }
                });
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
        session.execute_merging(
            Command::SetMesh {
                id,
                mesh: Some(MeshComponent {
                    source,
                    material: mesh.material,
                }),
            },
            Some(&format!("mesh-source:{id}")),
        );
    }
    if tile_finished {
        session.end_merge();
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

fn scene_settings(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &EditorCamera,
) {
    let project = state.project_settings.style_settings(session).look.clone();
    ui.vertical(|ui| {
        let before = session.scene.settings.clone();
        let mut look = before.look;
        let mut style = before.style.clone();
        let mut lighting = before.lighting;
        let mut finished = false;

        ui.label(egui::RichText::new("Look").strong());
        ui.horizontal(|ui| {
            for source in [LookSource::Project, LookSource::Scene] {
                if ui
                    .selectable_label(look == source, source.label())
                    .on_hover_text(match source {
                        LookSource::Project => {
                            "Use the look set in Project settings > Look, shared by every \
                             scene that uses it"
                        }
                        LookSource::Scene => "Give this scene its own look",
                    })
                    .clicked()
                    && look != source
                {
                    // Switching keeps what is on screen: the console
                    // settings are copied across, fog and background stay.
                    style = look_editor::with_scene_atmosphere(&project, &style);
                    look = source;
                    finished = true;
                }
            }
        });
        match look {
            LookSource::Project => {
                ui.label(
                    egui::RichText::new(format!(
                        "Uses the project look ({}). Fog and background below are this \
                         scene's own.",
                        project.preset().label()
                    ))
                    .small()
                    .color(theme::TEXT_DIM),
                );
                if ui.button("Edit the project look…").clicked() {
                    state.project_settings.page = crate::project_settings::Page::Look;
                    // The dock is out while panels draw: open it afterwards.
                    state.open_pane = Some(Pane::ProjectSettings);
                }
            }
            LookSource::Scene => {
                finished |= look_editor::console_look(ui, &mut style);
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Atmosphere").strong());
        finished |= look_editor::atmosphere(ui, &mut style);
        ui.separator();
        ui.label(egui::RichText::new("Lighting").strong());
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
        let look_changed = look != before.look;
        let style_changed = style != before.style;
        let lighting_changed = lighting != before.lighting;
        if (look_changed || style_changed || lighting_changed) && style.validate().is_ok() {
            session.execute_merging(
                Command::SetSettings {
                    look: look_changed.then_some(look),
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
        ui.separator();
        if ui.button("Use current view as start camera").clicked() {
            session.execute(Command::SetSettings {
                look: None,
                style: None,
                lighting: None,
                camera: Some(camera.to_start()),
            });
        }
    });
}

fn modals(
    ctx: &egui::Context,
    session: &mut EditorSession,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    if let Some(pending) = state.confirm.clone() {
        let scene = pending.leaves_scene() && session.is_dirty();
        let settings = pending.leaves_project() && state.project_settings.is_dirty();
        egui::Modal::new(egui::Id::unique("copperframe-confirm-close")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            let what = match (scene, settings) {
                (true, true) => format!(
                    "{} and the project settings have",
                    session.project.relative(&session.scene_path)
                ),
                (false, true) => "The project settings have".to_owned(),
                _ => format!("{} has", session.project.relative(&session.scene_path)),
            };
            ui.label(format!(
                "{what} unsaved changes. Save them before you {}?",
                pending.verb()
            ));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if scene {
                        save(session, state, false);
                    }
                    if settings {
                        state.project_settings.save(session);
                    }
                    if !state.loses_edits(session, &pending) {
                        actions.push(pending.clone());
                    }
                    state.confirm = None;
                }
                if ui.button("Don't save").clicked() {
                    actions.push(pending.clone());
                    state.confirm = None;
                }
                if ui.button("Cancel").clicked() {
                    state.confirm = None;
                }
            });
        });
    }
    scene_dialog(ctx, session, state, actions);
    if let Some(disk) = state.disk_changed {
        egui::Modal::new(egui::Id::unique("copperframe-disk-changed")).show(ctx, |ui| {
            ui.heading("Scene changed by another program");
            ui.label(format!(
                "{} was changed on disk (revision {disk}) while you have unsaved edits.",
                session.project.relative(&session.scene_path),
            ));
            ui.horizontal(|ui| {
                if ui.button("Reload from disk (discard my changes)").clicked() {
                    if let Err(e) = session.reload() {
                        session.error(e.to_string());
                    }
                    state.disk_changed = None;
                }
                if ui
                    .button("Keep my version")
                    .on_hover_text("Saving later asks whether to overwrite the other version")
                    .clicked()
                {
                    state.disk_changed = None;
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

/// File > Open Scene > Browse…: a file picker in the project's scenes
/// folder. Files outside it are refused.
fn pick_scene(session: &mut EditorSession) -> Option<PathBuf> {
    let dir = session
        .project
        .resolve(copperframe_assets::project::SCENES_DIR);
    let picked = rfd::FileDialog::new()
        .set_title("Open a scene")
        .set_directory(&dir)
        .add_filter("Scene", &["json"])
        .pick_file()?;
    let picked = dunce::canonicalize(&picked).unwrap_or(picked);
    let dir = dunce::canonicalize(&dir).unwrap_or(dir);
    let name = picked.to_string_lossy();
    if !picked.starts_with(&dir) || !name.ends_with(SCENE_SUFFIX) {
        session.error(format!(
            "{} is not a scene of this project (scenes are *{SCENE_SUFFIX} files in scenes/)",
            picked.display()
        ));
        return None;
    }
    let rel = picked.strip_prefix(&dir).ok()?;
    Some(
        session
            .project
            .resolve(copperframe_assets::project::SCENES_DIR)
            .join(rel),
    )
}

/// The New Scene and Save Scene As dialog.
fn scene_dialog(
    ctx: &egui::Context,
    session: &mut EditorSession,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let Some(dialog) = &mut state.scene_dialog else {
        return;
    };
    let folders = session.scene_folders();
    let mut close = false;
    let mut confirm = false;
    egui::Modal::new(egui::Id::unique("copperframe-scene-dialog")).show(ctx, |ui| {
        ui.heading(match dialog.kind {
            SceneDialogKind::New => "New scene",
            SceneDialogKind::SaveAs => "Save scene as",
        });
        egui::Grid::new("scene-dialog-grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Name");
                let edit = ui.text_edit_singleline(&mut dialog.name);
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if edit.changed() {
                    dialog.error = None;
                }
                ui.end_row();
                ui.label("Folder");
                egui::ComboBox::from_id_salt("scene-dialog-folder")
                    .selected_text(format!("{}/", dialog.folder))
                    .show_ui(ui, |ui| {
                        for f in &folders {
                            ui.selectable_value(&mut dialog.folder, f.clone(), format!("{f}/"));
                        }
                    });
                ui.end_row();
                if dialog.kind == SceneDialogKind::New {
                    ui.label("Start with");
                    ui.vertical(|ui| {
                        for t in SceneTemplate::ALL {
                            ui.radio_value(&mut dialog.template, t, t.label());
                        }
                    });
                    ui.end_row();
                }
            });
        ui.label(
            egui::RichText::new(format!(
                "File: {}/{}{SCENE_SUFFIX}",
                dialog.folder,
                scene_stem(&dialog.name)
            ))
            .weak(),
        );
        if dialog.kind == SceneDialogKind::New {
            ui.label(
                egui::RichText::new(
                    "Uses the project look, with the open scene's fog, background and lighting.",
                )
                .weak(),
            );
        } else {
            ui.label(
                egui::RichText::new(
                    "Saves the scene with your edits under the new name and keeps editing \
                     that copy. The original file stays as it was last saved.",
                )
                .weak(),
            );
        }
        if let Some(e) = &dialog.error {
            ui.colored_label(level_color(LogLevel::Error), e);
        }
        ui.horizontal(|ui| {
            let verb = match dialog.kind {
                SceneDialogKind::New => "Create",
                SceneDialogKind::SaveAs => "Save",
            };
            if ui.button(verb).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                confirm = true;
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
        });
    });
    if confirm {
        match session.scene_path_for(&dialog.folder, &dialog.name) {
            Err(e) => dialog.error = Some(e),
            Ok(path) => match dialog.kind {
                SceneDialogKind::New => {
                    let action = Action::NewScene {
                        path,
                        name: dialog.name.trim().to_owned(),
                        template: dialog.template,
                    };
                    state.scene_dialog = None;
                    // Asks about unsaved edits first.
                    state.request(session, action, actions);
                    return;
                }
                SceneDialogKind::SaveAs => match session.save_scene_as(&path) {
                    Ok(()) => {
                        close = true;
                        state.conflict = None;
                    }
                    Err(e) => dialog.error = Some(e),
                },
            },
        }
    }
    if close {
        state.scene_dialog = None;
    }
}
