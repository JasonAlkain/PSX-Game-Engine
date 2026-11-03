//! The project hub: the editor's start screen when no project is open.
//!
//! It lists recently opened projects, opens a project folder, and creates
//! projects from the templates in [`copperframe_assets::template`]. Opening
//! or creating a project hands a [`HubAction`] to the app, which loads the
//! project into the same window. The recent list lives in
//! `recent_projects.json` in the engine's data folder.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use copperframe_assets::project::PROJECT_FILE;
use copperframe_assets::template::{
    NewProject, ProjectTemplate, create_project, demo_source, engine_root,
};
use egui::{Color32, RichText};
use serde::{Deserialize, Serialize};

/// File name of the recent projects list in the engine's data folder.
pub const RECENT_FILE: &str = "recent_projects.json";
/// Most projects remembered.
const RECENT_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub name: String,
    /// Seconds since 1970 when the project was last opened.
    pub opened: u64,
}

/// Recently opened projects, newest first, plus the folder new projects
/// were last created in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentProjects {
    #[serde(default)]
    pub projects: Vec<RecentProject>,
    #[serde(default)]
    pub last_location: Option<PathBuf>,
}

impl RecentProjects {
    /// Reads the list; a missing or unreadable file is an empty list.
    pub fn load(file: &Path) -> Self {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> Result<(), String> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// Moves `path` to the top of the list (adding it if new).
    pub fn touch(&mut self, path: &Path, name: &str) {
        let path = canonical(path);
        self.projects.retain(|p| canonical(&p.path) != path);
        self.projects.insert(
            0,
            RecentProject {
                path,
                name: name.to_owned(),
                opened: now(),
            },
        );
        self.projects.truncate(RECENT_LIMIT);
    }

    pub fn remove(&mut self, path: &Path) {
        let path = canonical(path);
        self.projects.retain(|p| canonical(&p.path) != path);
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "3 minutes ago", "yesterday", ...
fn ago(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".into(),
        60..3600 => plural(s / 60, "minute"),
        3600..86_400 => plural(s / 3600, "hour"),
        86_400..172_800 => "yesterday".into(),
        _ => plural(s / 86_400, "day"),
    }
}

fn plural(n: u64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

/// A folder name for a project name: characters Windows does not allow
/// in file names become `_`, and trailing dots and spaces are dropped.
pub fn folder_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim_end_matches(['.', ' ']).to_owned();
    if s.is_empty() { "Untitled".into() } else { s }
}

/// Where new projects go when nothing was chosen before:
/// `Documents/Copperframe Projects`.
fn default_location() -> PathBuf {
    directories::UserDirs::new()
        .map(|u| {
            u.document_dir()
                .unwrap_or(u.home_dir())
                .join("Copperframe Projects")
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// What the hub asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum HubAction {
    /// Open this project folder in the editor.
    Open {
        path: PathBuf,
        steal_lock: bool,
    },
    Quit,
}

/// A failed open, shown on the hub.
#[derive(Debug, Clone)]
pub struct OpenFailure {
    pub path: PathBuf,
    pub message: String,
    /// The project is locked by another editor; offer "Open anyway".
    pub locked: bool,
}

struct NewProjectForm {
    name: String,
    location: String,
    template: ProjectTemplate,
    game_code: bool,
    error: Option<String>,
}

/// The hub's state.
pub struct Hub {
    pub recent: RecentProjects,
    /// Where [`Hub::recent`] is saved; `None` keeps it in memory.
    store: Option<PathBuf>,
    form: Option<NewProjectForm>,
    pub failure: Option<OpenFailure>,
    engine: Option<PathBuf>,
    demo: Option<PathBuf>,
    /// Checked when the new project form opens (it runs `cargo`).
    rust: Option<bool>,
    header: Option<egui::TextureHandle>,
}

impl Hub {
    /// A hub whose recent list is stored in `store`.
    pub fn new(store: Option<PathBuf>) -> Self {
        let recent = store
            .as_deref()
            .map(RecentProjects::load)
            .unwrap_or_default();
        Self {
            recent,
            store,
            form: None,
            failure: None,
            engine: engine_root(),
            demo: demo_source(),
            rust: None,
            header: None,
        }
    }

    /// The recent list in the engine's data folder.
    pub fn default_store() -> Option<PathBuf> {
        copperframe_core::report::data_dir()
            .or_else(copperframe_core::report::engine_data_dir)
            .map(|d| d.join(RECENT_FILE))
    }

    /// Records a project that was opened.
    pub fn opened(&mut self, path: &Path, name: &str) {
        self.recent.touch(path, name);
        self.failure = None;
        self.persist();
    }

    fn persist(&self) {
        if let Some(store) = &self.store
            && let Err(e) = self.recent.save(store)
        {
            tracing::warn!("could not save the recent projects list: {e}");
        }
    }

    /// Opens the new project form (File > New Project in the editor).
    pub fn start_new_project(&mut self) {
        let location = self
            .recent
            .last_location
            .clone()
            .unwrap_or_else(default_location);
        let rust = *self
            .rust
            .get_or_insert_with(copperframe_assets::game::rust_available);
        self.form = Some(NewProjectForm {
            name: "My Game".into(),
            location: location.display().to_string(),
            template: ProjectTemplate::Starter,
            game_code: self.engine.is_some() && rust,
            error: None,
        });
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) -> Option<HubAction> {
        let mut action = None;
        if self.header.is_none() {
            self.header = copperframe_runtime::branding::header_texture(ui.ctx());
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            if let Some(tex) = &self.header {
                let size = tex.size_vec2();
                let scale = (ui.available_width() * 0.5 / size.x)
                    .min(140.0 / size.y)
                    .min(1.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new((tex.id(), size * scale)));
                });
            } else {
                ui.vertical_centered(|ui| ui.heading("Copperframe Engine"));
            }
            ui.add_space(12.0);
            if let Some(failure) = self.failure.clone() {
                self.failure_banner(ui, &failure, &mut action);
                ui.add_space(8.0);
            }
            // Two columns, at most 1100 points wide, centred.
            let width = ui.available_width().min(1100.0);
            let margin = (ui.available_width() - width) / 2.0;
            ui.horizontal_top(|ui| {
                ui.add_space(margin);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.columns(2, |cols| {
                            self.recent_list(&mut cols[0], &mut action);
                            cols[1].with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                self.start_column(ui, &mut action);
                            });
                        });
                    },
                );
            });
        });
        self.new_project_modal(ui.ctx(), &mut action);
        action
    }

    fn failure_banner(
        &mut self,
        ui: &mut egui::Ui,
        failure: &OpenFailure,
        action: &mut Option<HubAction>,
    ) {
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(70, 32, 30))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Could not open the project").strong());
                ui.label(&failure.message);
                ui.horizontal(|ui| {
                    if failure.locked
                        && ui
                            .button("Open anyway")
                            .on_hover_text(
                                "Use this if the other editor crashed or was closed. If it is \
                                 still running, close it first: two editors would overwrite \
                                 each other's changes.",
                            )
                            .clicked()
                    {
                        *action = Some(HubAction::Open {
                            path: failure.path.clone(),
                            steal_lock: true,
                        });
                    }
                    if ui.button("Dismiss").clicked() {
                        self.failure = None;
                    }
                });
            });
    }

    fn recent_list(&mut self, ui: &mut egui::Ui, action: &mut Option<HubAction>) {
        ui.heading("Recent projects");
        ui.separator();
        if self.recent.projects.is_empty() {
            ui.label(
                RichText::new("No projects yet. Create one with New project, or open a folder.")
                    .weak(),
            );
            return;
        }
        let now = now();
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("recent")
            .show(ui, |ui| {
                for p in &self.recent.projects {
                    let exists = p.path.join(PROJECT_FILE).is_file();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.name).strong().size(15.0));
                            ui.label(RichText::new(ago(p.opened, now)).weak());
                        });
                        ui.label(RichText::new(p.path.display().to_string()).weak().small());
                        if !exists {
                            ui.colored_label(
                                Color32::from_rgb(230, 190, 60),
                                "Folder not found (moved or deleted?)",
                            );
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(exists, egui::Button::new("Open")).clicked() {
                                *action = Some(HubAction::Open {
                                    path: p.path.clone(),
                                    steal_lock: false,
                                });
                            }
                            if ui
                                .add_enabled(p.path.is_dir(), egui::Button::new("Show in folder"))
                                .clicked()
                                && let Err(e) = copperframe_core::report::open_external(
                                    &p.path.display().to_string(),
                                )
                            {
                                tracing::warn!("could not open the folder: {e}");
                            }
                            if ui
                                .button("Remove from list")
                                .on_hover_text("Forgets the project here; its files are kept")
                                .clicked()
                            {
                                remove = Some(p.path.clone());
                            }
                        });
                    });
                }
            });
        if let Some(path) = remove {
            self.recent.remove(&path);
            self.persist();
        }
    }

    fn start_column(&mut self, ui: &mut egui::Ui, action: &mut Option<HubAction>) {
        ui.heading("Start");
        ui.separator();
        let big = |text: &str| {
            egui::Button::new(RichText::new(text).size(16.0)).min_size(egui::vec2(220.0, 34.0))
        };
        if ui.add(big("New project…")).clicked() {
            self.start_new_project();
        }
        ui.label(RichText::new("Pick a name, a folder and a template.").weak());
        ui.add_space(8.0);
        if ui.add(big("Open project…")).clicked()
            && let Some(dir) = pick_folder("Open a Copperframe project", None)
        {
            *action = Some(self.open_folder(dir));
        }
        ui.label(RichText::new("Choose a folder that contains project.json.").weak());
        if let Some(demo) = self.demo.clone() {
            ui.add_space(8.0);
            if ui.add(big("Open the example project")).clicked() {
                *action = Some(HubAction::Open {
                    path: demo,
                    steal_lock: false,
                });
            }
            ui.label(
                RichText::new(
                    "The Lever Room, as shipped. To experiment freely, create a new project \
                     from the Lever Room template instead.",
                )
                .weak(),
            );
        }
        ui.add_space(16.0);
        if ui.button("Quit").clicked() {
            *action = Some(HubAction::Quit);
        }
    }

    /// The action for a folder the user picked: open it if it is a project,
    /// else explain.
    fn open_folder(&mut self, dir: PathBuf) -> HubAction {
        // Picking project.json's folder or the file itself both work.
        let dir = if dir.file_name().is_some_and(|n| n == PROJECT_FILE) {
            dir.parent().map(Path::to_path_buf).unwrap_or(dir)
        } else {
            dir
        };
        HubAction::Open {
            path: dir,
            steal_lock: false,
        }
    }

    fn new_project_modal(&mut self, ctx: &egui::Context, action: &mut Option<HubAction>) {
        let Some(form) = &mut self.form else { return };
        let mut close = false;
        let mut create = false;
        egui::Modal::new(egui::Id::unique("copperframe-new-project")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("New project");
            ui.add_space(4.0);
            egui::Grid::new("new-project")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(320.0));
                    ui.end_row();
                    ui.label("Location");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut form.location).desired_width(240.0));
                        if ui.button("Browse…").clicked()
                            && let Some(dir) = pick_folder(
                                "Where to create the project",
                                Some(Path::new(&form.location)),
                            )
                        {
                            form.location = dir.display().to_string();
                        }
                    });
                    ui.end_row();
                });
            let target = Path::new(form.location.trim()).join(folder_name(&form.name));
            ui.label(
                RichText::new(format!("The project folder will be {}", target.display())).weak(),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("Template").strong());
            for t in ProjectTemplate::ALL {
                let available =
                    !t.needs_game_code() || (self.demo.is_some() && self.engine.is_some());
                ui.add_enabled_ui(available, |ui| {
                    ui.radio_value(&mut form.template, t, t.label())
                        .on_disabled_hover_text(
                            "Needs the engine source and the example project: the demo's \
                             gameplay is Rust code",
                        );
                });
                ui.indent(t.id(), |ui| ui.label(RichText::new(t.description()).weak()));
            }
            ui.add_space(6.0);
            if form.template.needs_game_code() {
                form.game_code = true;
            }
            let can_toggle = self.engine.is_some() && !form.template.needs_game_code();
            ui.add_enabled_ui(can_toggle, |ui| {
                ui.checkbox(&mut form.game_code, "Include Rust game code")
                    .on_hover_text(
                        "Adds a game/ folder with a Rust crate where you write your own \
                         behaviours. Without it the project uses the built-in behaviours \
                         (first-person player, camera zones, spin) only.",
                    )
                    .on_disabled_hover_text(if self.engine.is_none() {
                        "Needs the engine source: this editor was not built from an engine \
                         checkout. Projects without game code still play and export."
                    } else {
                        "This template's gameplay is Rust code, so it always includes it."
                    });
            });
            if self.engine.is_none() {
                ui.label(
                    RichText::new(
                        "Rust game code needs the engine source (from GitHub); this copy can \
                         make projects with the built-in behaviours.",
                    )
                    .weak(),
                );
            } else if form.game_code && self.rust == Some(false) {
                rust_missing_note(ui);
            }
            if let Some(err) = &form.error {
                ui.add_space(4.0);
                ui.colored_label(Color32::from_rgb(240, 90, 80), err);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok = !form.name.trim().is_empty() && !form.location.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("Create")).clicked()
                    || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    create = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        if create {
            let location = PathBuf::from(form.location.trim());
            let options = NewProject {
                dir: location.join(folder_name(&form.name)),
                name: form.name.trim().to_owned(),
                template: form.template,
                game_code: form.game_code && self.engine.is_some(),
            };
            match create_project(&options) {
                Ok(created) => {
                    self.recent.last_location = Some(location);
                    self.persist();
                    *action = Some(HubAction::Open {
                        path: created.project.root,
                        steal_lock: false,
                    });
                    close = true;
                }
                Err(e) => form.error = Some(e),
            }
        }
        if close {
            self.form = None;
        }
    }
}

/// The note shown where Rust is needed but `cargo` was not found.
pub fn rust_missing_note(ui: &mut egui::Ui) {
    ui.colored_label(
        Color32::from_rgb(230, 190, 60),
        "Rust was not found on this computer. Game code is compiled with Rust, so Play and \
         Export will not work until it is installed.",
    );
    ui.horizontal(|ui| {
        ui.hyperlink_to(
            "Install Rust (rustup.rs)",
            copperframe_assets::game::RUST_INSTALL_URL,
        );
        ui.label(RichText::new("then restart the editor.").weak());
    });
}

/// A native folder picker. `None` if cancelled or unavailable.
pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(start) = start.filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder()
}

/// A native file picker for `extensions` (e.g. `["json"]`).
pub fn pick_file(
    title: &str,
    start: Option<&Path>,
    filter: &str,
    extensions: &[&str],
) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title(title)
        .add_filter(filter, extensions);
    if let Some(start) = start.filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_list_is_newest_first_without_duplicates() {
        let dir = std::env::temp_dir().join(format!("cf_recent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut r = RecentProjects::default();
        r.touch(&a, "A");
        r.touch(&b, "B");
        r.touch(&a.join("."), "A renamed");
        let names: Vec<&str> = r.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A renamed", "B"]);
        for i in 0..30 {
            r.touch(&dir.join(format!("p{i}")), "P");
        }
        assert_eq!(r.projects.len(), RECENT_LIMIT);

        let file = dir.join("store/recent.json");
        r.remove(&dir.join("p29"));
        r.last_location = Some(dir.clone());
        r.save(&file).unwrap();
        assert_eq!(RecentProjects::load(&file), r);
        assert_eq!(
            RecentProjects::load(&dir.join("nope.json")),
            RecentProjects::default()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_names_are_safe_on_windows() {
        assert_eq!(folder_name("My Game"), "My Game");
        assert_eq!(folder_name("What? A <Game>: 2"), "What_ A _Game__ 2");
        assert_eq!(folder_name("dots..."), "dots");
        assert_eq!(folder_name("   "), "Untitled");
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(0, 120), "2 minutes ago");
        assert_eq!(ago(0, 3600), "1 hour ago");
        assert_eq!(ago(0, 90_000), "yesterday");
        assert_eq!(ago(0, 86_400 * 5), "5 days ago");
    }
}
