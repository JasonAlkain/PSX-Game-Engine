//! The Project Settings panel (the toolbar's cog): settings that belong to
//! the whole game rather than one scene. They live in `project.json` (and
//! the default keys in `input.json`), so they travel with the project and
//! into exports.
//!
//! The panel edits a draft. Save checks it with the same rules as
//! `Project::open` and writes the files; Revert drops the draft. Project
//! settings are not part of the scene's undo history.

use std::path::{Path, PathBuf};

use copperframe_assets::input::{self, BUTTON_NAMES, InputMap};
use copperframe_assets::project::{
    AudioSettings, BusSettings, DisplaySettings, GraphicsStyle, MAX_GRAVITY, PhysicsSettings,
    ProjectFile, StyleSettings,
};
use copperframe_scene::{Command, LookSource};
use egui::{Color32, RichText};

use crate::audio_tools::EditorAudio;
use crate::gameplay::GameTools;
use crate::hub;
use crate::icons::{self, Icon};
use crate::session::EditorSession;
use crate::theme;

/// A page of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    General,
    Display,
    Look,
    Input,
    Scripting,
    Physics,
    Audio,
    Build,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::General,
        Page::Display,
        Page::Look,
        Page::Input,
        Page::Scripting,
        Page::Physics,
        Page::Audio,
        Page::Build,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Display => "Window",
            Page::Look => "Look",
            Page::Input => "Input",
            Page::Scripting => "Scripting",
            Page::Physics => "Physics",
            Page::Audio => "Audio",
            Page::Build => "Build",
        }
    }
}

/// Unsaved edits.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub file: ProjectFile,
    pub input: InputMap,
    /// What the files held when the draft was taken.
    base_file: ProjectFile,
    base_input: InputMap,
    /// `input.json` could not be read; saving would replace it.
    pub input_error: Option<String>,
}

impl Draft {
    pub fn load(session: &EditorSession) -> Draft {
        let (input, input_error) = match InputMap::load_project(&session.project.root) {
            Ok(m) => (m, None),
            Err(e) => (InputMap::default(), Some(e)),
        };
        Draft {
            file: session.project.file.clone(),
            base_file: session.project.file.clone(),
            input: input.clone(),
            base_input: input,
            input_error,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.file != self.base_file || self.input != self.base_input
    }

    fn input_dirty(&self) -> bool {
        self.input != self.base_input
    }

    /// Checks and writes the draft. On failure nothing changes in the
    /// session and the reason is returned.
    pub fn save(&mut self, session: &mut EditorSession) -> Result<(), String> {
        self.file.check()?;
        self.input.check()?;
        let scene = session.project.resolve(&self.file.startup_scene);
        if !scene.is_file() {
            return Err(format!(
                "the start scene {} does not exist",
                self.file.startup_scene
            ));
        }
        let previous = std::mem::replace(&mut session.project.file, self.file.clone());
        if let Err(e) = session.project.save() {
            session.project.file = previous;
            return Err(e.to_string());
        }
        if self.input_dirty() {
            self.input.save_project(&session.project.root)?;
            self.input_error = None;
        }
        session.project.file.version = copperframe_assets::project::PROJECT_VERSION;
        self.file.version = session.project.file.version;
        self.base_file = self.file.clone();
        self.base_input = self.input.clone();
        Ok(())
    }
}

/// The panel's state, kept in `UiState`.
#[derive(Debug, Default)]
pub struct ProjectSettingsPanel {
    pub page: Page,
    draft: Option<Draft>,
    /// Name typed for a new input action.
    new_action: String,
    /// Result of the last save, shown under the header.
    message: Option<(bool, String)>,
    /// Each scene's look source (relative path), and when it was read.
    looks: Option<(f64, Vec<(String, LookSource)>)>,
    /// "Make all use the project look" is asking for confirmation.
    confirm_migrate: bool,
    /// The Audio page asked for the Audio mixer panel.
    pub open_mixer: bool,
    /// The project's Lua scripts, and when the folder was read.
    scripts: Option<(f64, Vec<String>)>,
    /// The Scripting page asked for the Problems panel.
    pub open_problems: bool,
}

/// The look source of every scene file (unreadable files are skipped).
fn scene_looks(session: &EditorSession) -> Vec<(String, LookSource)> {
    session
        .scene_files()
        .into_iter()
        .filter_map(|p| {
            let look = if p == session.scene_path {
                session.scene.settings.look
            } else {
                copperframe_scene::load_scene(&p).ok()?.settings.look
            };
            Some((session.project.relative(&p), look))
        })
        .collect()
}

/// Switches the listed scenes to the saved project look, keeping their fog
/// and background (the open scene as an undoable edit). Returns how many
/// changed.
fn use_project_look(session: &mut EditorSession, scenes: &[String]) -> usize {
    let look = session.project.file.style.look.clone();
    let mut n = 0;
    for rel in scenes {
        let path = session.project.resolve(rel);
        if path == session.scene_path {
            let style =
                crate::look_editor::with_scene_atmosphere(&look, &session.scene.settings.style);
            if session
                .execute(Command::SetSettings {
                    look: Some(LookSource::Project),
                    style: Some(style),
                    lighting: None,
                    camera: None,
                })
                .is_some()
            {
                n += 1;
            }
            continue;
        }
        let mut scene = match copperframe_scene::load_scene(&path) {
            Ok(s) => s,
            Err(e) => {
                session.error(e.to_string());
                continue;
            }
        };
        scene.settings.style =
            crate::look_editor::with_scene_atmosphere(&look, &scene.settings.style);
        scene.settings.look = LookSource::Project;
        scene.revision += 1;
        match copperframe_scene::save_scene(&scene, &path) {
            Ok(()) => n += 1,
            Err(e) => session.error(e.to_string()),
        }
    }
    session.info(format!("{n} scenes now use the project look"));
    n
}

impl ProjectSettingsPanel {
    /// Saves the draft (the "unsaved changes" prompt). Failures are
    /// logged and shown in the panel.
    pub fn save(&mut self, session: &mut EditorSession) -> bool {
        match &mut self.draft {
            Some(draft) => save_draft(draft, session, &mut self.message),
            None => true,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(Draft::is_dirty)
    }

    /// The mixer to preview with: the draft's, so changes are heard
    /// before Save.
    pub fn audio_settings(&self, session: &EditorSession) -> AudioSettings {
        match &self.draft {
            Some(d) => d.file.audio,
            None => session.project.file.audio,
        }
    }

    /// The Audio mixer panel. It edits the same draft as this panel, so
    /// either one's Save writes both.
    pub fn mixer(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        tools: &mut GameTools,
        audio: &mut EditorAudio,
    ) {
        self.refresh_draft(session);
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        ui.horizontal(|ui| {
            let rect = icons::row_rect(ui, 16.0);
            icons::paint(ui.painter(), rect, Icon::Sound, theme::ACCENT);
            ui.label(RichText::new("Audio mixer").strong());
            ui.label(
                RichText::new("project mixer: saved in project.json, used by Play and Export")
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.save_revert(ui, session, tools, &mut draft);
            });
        });
        if !ui.horizontal(|ui| audio.unavailable(ui)).inner {
            hint(
                ui,
                "Meters show sound previews (double-click a sound in Assets). Solo is for \
                 listening here and is not saved. A running game keeps the mixer it started \
                 with.",
            );
        }
        if let Some(problem) = draft.file.audio.problem() {
            ui.colored_label(
                crate::ui::level_color(crate::session::LogLevel::Error),
                problem,
            );
        }
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                audio.strips(ui, &mut draft.file.audio);
            });
        if let Some(problem) = draft.file.audio.problem() {
            ui.colored_label(
                crate::ui::level_color(crate::session::LogLevel::Error),
                problem,
            );
        }
        self.draft = Some(draft);
    }

    /// Loads the draft if there is none, or if it has no edits of its own
    /// and the project changed under it (File > Set as Startup Scene).
    fn refresh_draft(&mut self, session: &EditorSession) {
        let stale = match &self.draft {
            None => true,
            Some(d) => !d.is_dirty() && d.base_file != session.project.file,
        };
        if stale {
            self.draft = Some(Draft::load(session));
        }
    }

    /// The look settings to preview: the draft's while the panel has
    /// unsaved edits, so changes show in the viewport before Save.
    pub fn style_settings<'a>(&'a self, session: &'a EditorSession) -> &'a StyleSettings {
        match &self.draft {
            Some(d) => &d.file.style,
            None => &session.project.file.style,
        }
    }

    /// The Look page: the player's PS1/PS2 choice, the project look, and
    /// which scenes use it.
    fn look(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, draft: &mut Draft) {
        look(ui, draft);
        ui.separator();
        ui.label(RichText::new("Project look").strong());
        hint(
            ui,
            "The console look of every scene set to \"Project look\" in Scene settings: \
             resolution, wobble, textures, shading and shadows. Each scene keeps its own \
             fog, background and lighting.",
        );
        let mut style = draft.file.style.look.clone();
        crate::look_editor::console_look(ui, &mut style);
        if style != draft.file.style.look && style.validate().is_ok() {
            draft.file.style.look = style;
        }
        ui.horizontal(|ui| {
            if ui
                .button("Copy from the open scene")
                .on_hover_text("Take the open scene's look as the project look")
                .clicked()
            {
                draft.file.style.look = session.scene_style();
            }
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Scenes").strong());
        let now = ui.input(|i| i.time);
        if self.looks.as_ref().is_none_or(|(t, _)| now - t > 2.0) {
            self.looks = Some((now, scene_looks(session)));
        }
        let own: Vec<String> = self
            .looks
            .as_ref()
            .map(|(_, l)| {
                l.iter()
                    .filter(|(_, s)| *s == LookSource::Scene)
                    .map(|(p, _)| p.clone())
                    .collect()
            })
            .unwrap_or_default();
        let total = self.looks.as_ref().map_or(0, |(_, l)| l.len());
        if own.is_empty() {
            let text = if total == 1 {
                "The scene uses the project look.".to_owned()
            } else {
                format!("All {total} scenes use the project look.")
            };
            hint(ui, &text);
            return;
        }
        hint(
            ui,
            &format!(
                "{} of {total} scenes have their own look (scenes made before project looks \
                 keep theirs until you change them):",
                own.len()
            ),
        );
        for rel in &own {
            ui.label(RichText::new(format!("  {rel}")).color(theme::TEXT_DIM));
        }
        let saved = draft.file.style.look == draft.base_file.style.look;
        let r = ui
            .add_enabled(
                saved,
                egui::Button::new(format!("Make all {} use the project look…", own.len())),
            )
            .on_hover_text(
                "Their own console settings are replaced by the project look; fog, \
                 background and lighting are kept",
            )
            .on_disabled_hover_text("Save the project settings first");
        if r.clicked() {
            self.confirm_migrate = true;
        }
        if self.confirm_migrate {
            ui.horizontal(|ui| {
                ui.colored_label(
                    crate::ui::level_color(crate::session::LogLevel::Warning),
                    "Replace their look settings?",
                );
                if ui.button("Replace").clicked() {
                    let n = use_project_look(session, &own);
                    self.message = Some((true, format!("{n} scenes now use the project look")));
                    self.looks = None;
                    self.confirm_migrate = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_migrate = false;
                }
            });
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, tools: &mut GameTools) {
        self.refresh_draft(session);
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        self.header(ui, session, tools, &mut draft);
        ui.separator();
        let wide = ui.available_width() >= 430.0;
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(104.0);
                    self.nav(ui, &draft);
                });
                ui.separator();
                ui.vertical(|ui| self.page(ui, session, tools, &mut draft));
            });
        } else {
            ui.horizontal_wrapped(|ui| self.nav(ui, &draft));
            ui.separator();
            self.page(ui, session, tools, &mut draft);
        }
        self.draft = Some(draft);
    }

    fn header(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        tools: &mut GameTools,
        draft: &mut Draft,
    ) {
        ui.horizontal(|ui| {
            let rect = icons::row_rect(ui, 16.0);
            icons::paint(ui.painter(), rect, Icon::Cog, theme::ACCENT);
            ui.label(RichText::new("Project settings").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.save_revert(ui, session, tools, draft);
            });
        });
        if let Some((ok, text)) = &self.message {
            let color = if *ok {
                Color32::from_rgb(120, 210, 120)
            } else {
                crate::ui::level_color(crate::session::LogLevel::Error)
            };
            ui.colored_label(color, text);
        }
    }

    /// Save and Revert (right to left) and the unsaved mark.
    fn save_revert(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        tools: &mut GameTools,
        draft: &mut Draft,
    ) {
        let dirty = draft.is_dirty();
        let scripting = session.project.file.scripting;
        if ui
            .add_enabled(dirty, egui::Button::new("Save"))
            .on_hover_text("Write project.json (and input.json if the keys changed)")
            .clicked()
            && save_draft(draft, session, &mut self.message)
        {
            tools.export_dir = session.project.export_dir().display().to_string();
            if session.project.file.scripting != scripting {
                // Scripts were switched on or off: their kinds change.
                tools.refresh_schema(session);
            }
        }
        if ui
            .add_enabled(dirty, egui::Button::new("Revert"))
            .on_hover_text("Drop the unsaved project settings")
            .clicked()
        {
            *draft = Draft::load(session);
            self.message = None;
        }
        if dirty {
            ui.label(RichText::new("unsaved").color(theme::ACCENT));
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui, draft: &Draft) {
        for page in Page::ALL {
            let mut text = RichText::new(page.title());
            if page == Page::Input && draft.input_error.is_some() {
                text = text.color(crate::ui::level_color(crate::session::LogLevel::Warning));
            }
            let selected = self.page == page;
            let button = egui::Button::selectable(selected, text);
            // In a row the buttons take their own width so the row can
            // wrap; in the side column they share one width.
            let r = if ui.layout().is_horizontal() {
                ui.add(button.min_size(egui::vec2(0.0, 22.0)))
            } else {
                ui.add_sized([100.0, 22.0], button)
            };
            if r.clicked() {
                self.page = page;
            }
        }
    }

    fn page(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        tools: &mut GameTools,
        draft: &mut Draft,
    ) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.page {
                Page::Scripting => self.scripting(ui, session, tools, draft),
                Page::General => general(ui, session, draft),
                Page::Display => display(ui, &mut draft.file.display),
                Page::Look => self.look(ui, session, draft),
                Page::Input => self.input(ui, draft),
                Page::Physics => physics(ui, &mut draft.file.physics),
                Page::Audio => self.audio(ui, draft),
                Page::Build => build(ui, session, draft),
            });
    }

    fn scripting(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        tools: &mut GameTools,
        draft: &mut Draft,
    ) {
        use copperframe_assets::scripts::{SCRIPTS_DIR, find_lua_scripts, lua_kind_for};
        hint(
            ui,
            "Gameplay code can be Rust (the project's game/ crate) or scripts in the \
             scripts/ folder. Each script is a behaviour you add to entities with Add \
             component, next to the Rust ones.",
        );
        ui.checkbox(&mut draft.file.scripting.lua, "Lua scripts")
            .on_hover_text("Load scripts/**/*.lua as behaviours (Lua 5.4, built into every game)");
        hint(
            ui,
            "scripts/door.lua becomes the behaviour lua.door, scripts/puzzles/door.lua \
             lua.puzzles.door. Name scripts and folders with a-z, 0-9 and _.",
        );
        let saved = session.project.file.scripting.lua;
        if draft.file.scripting.lua != saved {
            ui.label(
                RichText::new(
                    "Save to apply; scenes that use Lua behaviours do not play with Lua off.",
                )
                .color(theme::ACCENT),
            );
        }
        ui.add_space(6.0);
        ui.separator();
        let now = ui.input(|i| i.time);
        if self.scripts.as_ref().is_none_or(|(t, _)| now - t > 2.0) {
            self.scripts = Some((now, find_lua_scripts(&session.project.root)));
        }
        let scripts = self
            .scripts
            .as_ref()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Scripts ({})", scripts.len())).strong());
            if ui
                .add_enabled(saved, egui::Button::new("New Lua script…"))
                .on_disabled_hover_text("Turn Lua on and save first")
                .clicked()
            {
                tools.new_script = Some(crate::scripting::NewScript::new(session.selection));
                self.scripts = None;
            }
            if ui.button("Open folder").clicked() {
                let dir = session.project.resolve(SCRIPTS_DIR);
                if let Err(e) = std::fs::create_dir_all(&dir)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        copperframe_core::report::open_external(&dir.display().to_string())
                            .map_err(|e| e.to_string())
                    })
                {
                    session.error(format!("could not open {SCRIPTS_DIR}/: {e}"));
                }
            }
        });
        if scripts.is_empty() {
            hint(ui, "No scripts yet.");
        }
        let mut open = None;
        egui::Grid::new("project-scripts")
            .num_columns(3)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for rel in &scripts {
                    let file = format!("{SCRIPTS_DIR}/{rel}");
                    ui.label(RichText::new(&file).monospace());
                    let broken = tools.problems.all().filter(|p| p.file == file).count();
                    if broken > 0 {
                        let r = ui.add(
                            egui::Button::new(
                                RichText::new(format!("{broken} problem(s)")).color(
                                    crate::ui::level_color(crate::session::LogLevel::Warning),
                                ),
                            )
                            .frame(false),
                        );
                        if r.on_hover_text("Show the Problems panel").clicked() {
                            self.open_problems = true;
                        }
                    } else {
                        ui.label(RichText::new(lua_kind_for(rel)).weak());
                    }
                    if ui.small_button("Open").clicked() {
                        open = Some(file);
                    }
                    ui.end_row();
                }
            });
        if let Some(file) = open {
            crate::scripting::open_in_editor(session, &file);
        }
    }

    fn input(&mut self, ui: &mut egui::Ui, draft: &mut Draft) {
        hint(
            ui,
            "The keys a new player starts with. Players can still change them in the \
             game's settings screen.",
        );
        if let Some(e) = &draft.input_error {
            ui.colored_label(
                crate::ui::level_color(crate::session::LogLevel::Warning),
                format!("{e}. Showing the defaults; saving replaces the file."),
            );
        }
        let mut remove = None;
        let mut bind: Option<(String, String)> = None;
        egui::Grid::new("project-input")
            .num_columns(2)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (action, buttons) in draft.input.actions.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.label(action.as_str());
                        if !input::is_default_action(action)
                            && ui
                                .small_button("×")
                                .on_hover_text("Remove this action")
                                .clicked()
                        {
                            remove = Some(action.clone());
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        buttons.retain(|b| {
                            !ui.button(format!("{}  ×", input::button_label(b)))
                                .on_hover_text("Click to unbind")
                                .clicked()
                        });
                        egui::ComboBox::from_id_salt(("bind", action.as_str()))
                            .selected_text("+ key")
                            .width(70.0)
                            .height(320.0)
                            .show_ui(ui, |ui| {
                                for name in BUTTON_NAMES {
                                    if buttons.iter().any(|b| b == name) {
                                        continue;
                                    }
                                    if ui
                                        .selectable_label(false, input::button_label(name))
                                        .clicked()
                                    {
                                        bind = Some((action.clone(), (*name).to_owned()));
                                    }
                                }
                            });
                    });
                    ui.end_row();
                }
            });
        if let Some((action, button)) = bind {
            let moved = draft.input.bind(&action, &button);
            if !moved.is_empty() {
                self.message = Some((
                    true,
                    format!(
                        "{} moved to {action} from {}",
                        input::button_label(&button),
                        moved.join(", ")
                    ),
                ));
            }
        }
        if let Some(action) = remove {
            draft.input.actions.remove(&action);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_action)
                    .hint_text("new_action")
                    .desired_width(140.0),
            );
            let name = self.new_action.trim().to_owned();
            let ok = input::valid_action_name(&name) && !draft.input.actions.contains_key(&name);
            if ui
                .add_enabled(ok, egui::Button::new("Add action"))
                .on_hover_text("Letters, digits, _ and . only. Game code reads it by this name")
                .clicked()
            {
                draft.input.actions.insert(name, Vec::new());
                self.new_action.clear();
            }
        });
        if ui
            .button("Reset to engine defaults")
            .on_hover_text("The engine's standard keys; custom actions are removed")
            .clicked()
        {
            draft.input = InputMap::default();
        }
    }
}

impl ProjectSettingsPanel {
    /// The Audio page: a summary of the mixer and a way to the full panel.
    fn audio(&mut self, ui: &mut egui::Ui, draft: &mut Draft) {
        hint(
            ui,
            "The project mixer: a master bus and Music, SFX, Voice and Ambient buses that \
             every Audio source plays through (choose a source's bus in its inspector). \
             Players' volume sliders apply on top.",
        );
        let a = &mut draft.file.audio;
        egui::Grid::new("project-audio")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let rows = [
                    ("Master", &mut a.master),
                    ("Music", &mut a.music),
                    ("SFX", &mut a.effects),
                    ("Voice", &mut a.voice),
                    ("Ambient", &mut a.ambient),
                ];
                for (name, bus) in rows {
                    ui.label(name);
                    ui.add(
                        egui::DragValue::new(&mut bus.volume_db)
                            .range(BusSettings::DB_RANGE)
                            .speed(0.2)
                            .max_decimals(1)
                            .suffix(" dB"),
                    );
                    ui.checkbox(&mut bus.mute, "Mute");
                    ui.end_row();
                }
            });
        ui.checkbox(&mut a.limiter, "Limiter on the master bus")
            .on_hover_text("Pulls peaks down before they clip");
        ui.add_space(6.0);
        if ui
            .button("Open the Audio mixer")
            .on_hover_text("Faders, meters, mute and solo")
            .clicked()
        {
            self.open_mixer = true;
        }
    }
}

fn save_draft(
    draft: &mut Draft,
    session: &mut EditorSession,
    message: &mut Option<(bool, String)>,
) -> bool {
    match draft.save(session) {
        Ok(()) => {
            session.info("saved the project settings");
            *message = Some((true, "Saved. Play and Export use them.".into()));
            true
        }
        Err(e) => {
            session.error(format!("project settings not saved: {e}"));
            *message = Some((false, e));
            false
        }
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
    ui.add_space(4.0);
}

fn general(ui: &mut egui::Ui, session: &EditorSession, draft: &mut Draft) {
    egui::Grid::new("project-general")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut draft.file.name).desired_width(220.0));
            ui.end_row();
            ui.label("Start scene");
            let scenes = session.scene_files();
            egui::ComboBox::from_id_salt("project-start-scene")
                .selected_text(draft.file.startup_scene.as_str())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for path in scenes {
                        let rel = session.project.relative(&path);
                        ui.selectable_value(&mut draft.file.startup_scene, rel.clone(), rel);
                    }
                });
            ui.end_row();
            ui.label("Folder");
            ui.horizontal(|ui| {
                ui.label(RichText::new(session.project.root.display().to_string()).weak());
                if ui.small_button("Open").clicked()
                    && let Err(e) = copperframe_core::report::open_external(
                        &session.project.root.display().to_string(),
                    )
                {
                    tracing::warn!("could not open the project folder: {e}");
                }
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    hint(
        ui,
        "The name is shown in the hub and the status bar, and names the default export \
         folder. The game starts with the start scene.",
    );
}

/// Common window sizes (4:3 first, like the consoles).
const SIZES: &[(u32, u32)] = &[
    (640, 480),
    (960, 720),
    (1280, 960),
    (1280, 720),
    (1600, 900),
    (1920, 1080),
];

fn display(ui: &mut egui::Ui, d: &mut DisplaySettings) {
    egui::Grid::new("project-display")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Window size");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.width).range(DisplaySettings::WIDTH_RANGE));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut d.height).range(DisplaySettings::HEIGHT_RANGE));
                egui::ComboBox::from_id_salt("project-size")
                    .selected_text("Presets")
                    .show_ui(ui, |ui| {
                        for &(w, h) in SIZES {
                            if ui
                                .selectable_label(
                                    d.width == w && d.height == h,
                                    format!("{w} × {h}"),
                                )
                                .clicked()
                            {
                                d.width = w;
                                d.height = h;
                            }
                        }
                    });
            });
            ui.end_row();
            ui.label("Start fullscreen");
            ui.checkbox(&mut d.fullscreen, "");
            ui.end_row();
            ui.label("Vertical sync");
            ui.checkbox(&mut d.vsync, "");
            ui.end_row();
        });
    ui.add_space(6.0);
    hint(
        ui,
        "The size of the game window, in points (it scales with the desktop). Fullscreen and \
         vsync are where a new player starts; they can change them in the game's settings. \
         The low internal resolution is part of each scene's style (Scene settings).",
    );
}

/// Gravity presets (m/s²).
const GRAVITY_PRESETS: &[(&str, f32)] = &[
    ("Earth", -9.81),
    ("Moon", -1.62),
    ("Mars", -3.71),
    ("Floaty", -4.0),
    ("Heavy", -20.0),
    ("None", 0.0),
];

fn physics(ui: &mut egui::Ui, p: &mut PhysicsSettings) {
    egui::Grid::new("project-physics")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Gravity (m/s²)");
            ui.horizontal(|ui| {
                for (v, axis) in [
                    (&mut p.gravity.x, "x"),
                    (&mut p.gravity.y, "y"),
                    (&mut p.gravity.z, "z"),
                ] {
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.05)
                            .range(-MAX_GRAVITY..=MAX_GRAVITY)
                            .prefix(format!("{axis} "))
                            .max_decimals(3),
                    );
                }
                egui::ComboBox::from_id_salt("project-gravity")
                    .selected_text("Presets")
                    .show_ui(ui, |ui| {
                        for &(name, y) in GRAVITY_PRESETS {
                            let g = glam::Vec3::new(0.0, y, 0.0);
                            if ui
                                .selectable_label(p.gravity == g, format!("{name} ({y})"))
                                .clicked()
                            {
                                p.gravity = g;
                            }
                        }
                    });
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    hint(
        ui,
        "Pulls every dynamic rigidbody each tick; a rigidbody's gravity scale multiplies it. \
         Earth is (0, -9.81, 0). Game code can change it while playing.",
    );
}

fn look(ui: &mut egui::Ui, draft: &mut Draft) {
    let s = &mut draft.file.style;
    ui.label("Default look");
    ui.horizontal(|ui| {
        for g in GraphicsStyle::ALL {
            ui.selectable_value(&mut s.default, g, g.label())
                .on_hover_text(match g {
                    GraphicsStyle::Scene => "Each scene's own style from Scene settings",
                    GraphicsStyle::Ps1 => {
                        "The PS1 preset: wobbly vertices, affine textures, no filtering"
                    }
                    GraphicsStyle::Ps2 => "The PS2 preset: filtered, mipmapped, per-pixel light",
                });
        }
    });
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.player_can_change,
        "Players can switch between PS1 and PS2 in the game's settings",
    );
    ui.add_space(6.0);
    hint(
        ui,
        "A preset keeps each scene's fog and background colour. Untick the box to lock the \
         game to the default look.",
    );
}

fn build(ui: &mut egui::Ui, session: &EditorSession, draft: &mut Draft) {
    let b = &mut draft.file.build;
    let root = &session.project.root;
    ui.label("Export folder");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut b.export_dir)
                .hint_text(format!(
                    "dist/{}",
                    copperframe_assets::project::file_slug(&draft.file.name)
                ))
                .desired_width(240.0),
        );
        if ui.button("Browse…").clicked()
            && let Some(dir) = hub::pick_folder("Export the game into this folder", Some(root))
        {
            b.export_dir = relative_or_absolute(root, &dir);
        }
    });
    hint(
        ui,
        "Relative to the project folder, or a full path. Empty: dist/ and the project name.",
    );
    ui.checkbox(
        &mut b.check_after_export,
        "Check the exported game starts (runs it headless from its folder)",
    );
    ui.add_enabled_ui(b.check_after_export, |ui| {
        ui.horizontal(|ui| {
            ui.label("Check with");
            let label = if b.check_script.is_empty() {
                "Idle run (5 seconds)".to_owned()
            } else {
                b.check_script.clone()
            };
            egui::ComboBox::from_id_salt("project-check-script")
                .selected_text(label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.check_script, String::new(), "Idle run (5 seconds)");
                    for script in GameTools::test_scripts(session) {
                        let rel = session.project.relative(&script);
                        ui.selectable_value(&mut b.check_script, rel.clone(), rel);
                    }
                });
        });
    });
}

/// `dir` relative to the project with `/` separators when it is inside
/// it, else the full path.
fn relative_or_absolute(root: &Path, dir: &Path) -> String {
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let dir: PathBuf = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    match dir.strip_prefix(&root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copperframe_assets::project::Project;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf_psettings_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn saving_a_draft_writes_the_project_and_input_files() {
        let root = temp("save");
        Project::create(&root, "Draft test").unwrap();
        let mut session = EditorSession::open(&root, None, false, false).unwrap();
        let mut draft = Draft::load(&session);
        assert!(!draft.is_dirty());
        assert!(draft.input_error.is_none());

        // Settings only: input.json is not created.
        draft.file.display.width = 640;
        draft.file.display.height = 480;
        draft.file.style.default = GraphicsStyle::Ps2;
        assert!(draft.is_dirty());
        draft.save(&mut session).unwrap();
        assert!(!draft.is_dirty());
        assert!(!InputMap::path(&root).exists());
        let on_disk = Project::open(&root).unwrap().file;
        assert_eq!(on_disk.display.width, 640);
        assert_eq!(on_disk.style.default, GraphicsStyle::Ps2);
        assert_eq!(session.project.file, on_disk);

        // Keys: written to input.json, where the game reads them.
        draft.input.bind("jump", "KeyJ");
        draft
            .input
            .actions
            .insert("crouch".into(), vec!["KeyC".into()]);
        draft.save(&mut session).unwrap();
        let input = InputMap::load_project(&root).unwrap();
        assert!(input.actions["jump"].contains(&"KeyJ".to_owned()));
        assert_eq!(input.actions["crouch"], vec!["KeyC"]);

        // Invalid drafts change nothing.
        draft.file.display.width = 1;
        assert!(draft.save(&mut session).is_err());
        draft.file.display.width = 640;
        draft.file.startup_scene = "scenes/missing.scene.json".into();
        assert!(
            draft
                .save(&mut session)
                .unwrap_err()
                .contains("does not exist")
        );
        assert_eq!(Project::open(&root).unwrap().file, on_disk);
        assert_eq!(session.project.file, on_disk);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_damaged_input_file_is_reported_not_lost() {
        let root = temp("bad_input");
        Project::create(&root, "T").unwrap();
        std::fs::write(InputMap::path(&root), "{ broken").unwrap();
        let session = EditorSession::open(&root, None, false, false).unwrap();
        let draft = Draft::load(&session);
        assert!(draft.input_error.is_some());
        assert_eq!(draft.input, InputMap::default());
        assert!(
            !draft.is_dirty(),
            "nothing is rewritten until the user saves"
        );
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn export_folders_inside_the_project_are_stored_relative() {
        let root = temp("rel");
        std::fs::create_dir_all(root.join("out/game")).unwrap();
        assert_eq!(
            relative_or_absolute(&root, &root.join("out/game")),
            "out/game"
        );
        let elsewhere = std::env::temp_dir();
        assert_eq!(
            relative_or_absolute(&root, &elsewhere),
            dunce::canonicalize(&elsewhere)
                .unwrap()
                .display()
                .to_string()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
