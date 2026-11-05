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
use copperframe_assets::project::{DisplaySettings, GraphicsStyle, ProjectFile};
use egui::{Color32, RichText};

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
    Build,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::General,
        Page::Display,
        Page::Look,
        Page::Input,
        Page::Build,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Display => "Window",
            Page::Look => "Look",
            Page::Input => "Input",
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

    pub fn show(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, tools: &mut GameTools) {
        // Without edits of its own the draft follows the project, which
        // File > Set as Startup Scene can change.
        let stale = match &self.draft {
            None => true,
            Some(d) => !d.is_dirty() && d.base_file != session.project.file,
        };
        if stale {
            self.draft = Some(Draft::load(session));
        }
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
                ui.vertical(|ui| self.page(ui, session, &mut draft));
            });
        } else {
            ui.horizontal_wrapped(|ui| self.nav(ui, &draft));
            ui.separator();
            self.page(ui, session, &mut draft);
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
                let dirty = draft.is_dirty();
                if ui
                    .add_enabled(dirty, egui::Button::new("Save"))
                    .on_hover_text("Write project.json (and input.json if the keys changed)")
                    .clicked()
                    && save_draft(draft, session, &mut self.message)
                {
                    tools.export_dir = session.project.export_dir().display().to_string();
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Revert"))
                    .on_hover_text("Drop the unsaved changes")
                    .clicked()
                {
                    *draft = Draft::load(session);
                    self.message = None;
                }
                if dirty {
                    ui.label(RichText::new("unsaved").color(theme::ACCENT));
                }
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

    fn nav(&mut self, ui: &mut egui::Ui, draft: &Draft) {
        for page in Page::ALL {
            let mut text = RichText::new(page.title());
            if page == Page::Input && draft.input_error.is_some() {
                text = text.color(crate::ui::level_color(crate::session::LogLevel::Warning));
            }
            let selected = self.page == page;
            let r = ui.add_sized(
                [
                    if ui.layout().is_horizontal() {
                        0.0
                    } else {
                        100.0
                    },
                    22.0,
                ],
                egui::Button::selectable(selected, text),
            );
            if r.clicked() {
                self.page = page;
            }
        }
    }

    fn page(&mut self, ui: &mut egui::Ui, session: &EditorSession, draft: &mut Draft) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.page {
                Page::General => general(ui, session, draft),
                Page::Display => display(ui, &mut draft.file.display),
                Page::Look => look(ui, draft),
                Page::Input => self.input(ui, draft),
                Page::Build => build(ui, session, draft),
            });
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
