//! Gameplay scripts in the editor: the Problems panel, the New script
//! dialog (Lua or C#), and opening a script in the user's text editor.
//!
//! Problems come from two places. Scripts that do not load are listed in
//! the game's behaviour schema (`problems`), read whenever the schema is.
//! Errors raised while the game runs arrive during Play as
//! `copperframe_script_problem` lines on the game's stdout
//! ([`copperframe_assets::play_control::PlayMessage`]) and stay listed until
//! the next Play.

use copperframe_assets::csharp::{
    self, CS_KIND_PREFIX, create_cs_script, cs_kind_for, new_cs_script_path,
};
use copperframe_assets::project::Project;
use copperframe_assets::scripts::{
    self, LUA_KIND_PREFIX, SCRIPTS_DIR, ScriptDiagnostic, create_lua_script, lua_kind_for,
    new_script_path,
};
use copperframe_core::EntityId;
use copperframe_scene::{BehaviorComponent, Command};
use egui::{Color32, RichText};
use serde_json::Value;

use crate::gameplay::GameTools;
use crate::session::EditorSession;
use crate::theme;

/// Lines shown above and below the failing line.
const EXCERPT_CONTEXT: u32 = 2;

/// The problems the Problems panel lists.
#[derive(Debug, Default)]
pub struct Problems {
    /// Scripts that did not load (from the behaviour schema, or the game
    /// at the start of Play).
    load: Vec<ScriptDiagnostic>,
    /// Errors raised while the last Play ran.
    runtime: Vec<ScriptDiagnostic>,
    /// Set when a problem not seen before arrives; the app opens the panel.
    pub arrived: bool,
}

impl Problems {
    /// Replaces the load problems with the schema's. Returns how many.
    pub fn set_load(&mut self, schema: &Value) -> usize {
        let new: Vec<ScriptDiagnostic> = schema["problems"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| serde_json::from_value(p.clone()).ok())
            .collect();
        if new.iter().any(|p| !self.load.contains(p)) {
            self.arrived = true;
        }
        self.load = new;
        self.load.len()
    }

    /// Adds a problem the running game reported.
    pub fn add_runtime(&mut self, problem: ScriptDiagnostic) {
        let list = if problem.is_runtime() {
            &mut self.runtime
        } else {
            &mut self.load
        };
        if !list.contains(&problem) {
            list.push(problem);
            self.arrived = true;
        }
    }

    pub fn clear_runtime(&mut self) {
        self.runtime.clear();
    }

    pub fn len(&self) -> usize {
        self.load.len() + self.runtime.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every problem: load problems first, then runtime ones in the order
    /// they happened.
    pub fn all(&self) -> impl Iterator<Item = &ScriptDiagnostic> {
        self.load.iter().chain(&self.runtime)
    }
}

/// Opens a project file in the program the system uses for it.
pub fn open_in_editor(session: &mut EditorSession, rel: &str) {
    let path = session.project.resolve(rel);
    if !path.is_file() {
        session.error(format!("{rel} does not exist"));
        return;
    }
    if let Err(e) = copperframe_core::report::open_external(&path.display().to_string()) {
        session.error(format!("could not open {rel}: {e}"));
    }
}

/// Whether the project turns on any scripting language.
pub fn scripting_on(project: &Project) -> bool {
    project.file.scripting.lua || project.file.scripting.csharp
}

/// Whether the project has scripts in a language it turns on.
pub fn has_scripts(project: &Project) -> bool {
    (project.file.scripting.lua && !scripts::find_lua_scripts(&project.root).is_empty())
        || (project.file.scripting.csharp && !csharp::find_cs_scripts(&project.root).is_empty())
}

/// Whether a changed project file is a script in a language that is on.
pub fn is_script_change(project: &Project, rel: &str) -> bool {
    rel.starts_with(&format!("{SCRIPTS_DIR}/"))
        && ((project.file.scripting.lua && rel.ends_with(".lua"))
            || (project.file.scripting.csharp && rel.ends_with(".cs")))
}

/// The project-relative file of a script behaviour kind, if it exists.
/// C# files are found by the class-named-after-file convention.
pub fn script_file(session: &EditorSession, kind: &str) -> Option<String> {
    let rel = match scripts::lua_script_for(kind) {
        Some(rel) => rel,
        None => csharp::cs_script_for(&session.project.root, kind)?,
    };
    let rel = format!("{SCRIPTS_DIR}/{rel}");
    session.project.resolve(&rel).is_file().then_some(rel)
}

/// Up to two lines either side of `line` (1-based) of a project file.
pub fn excerpt(session: &EditorSession, rel: &str, line: u32) -> Vec<(u32, String)> {
    let Ok(text) = std::fs::read_to_string(session.project.resolve(rel)) else {
        return Vec::new();
    };
    let first = line.saturating_sub(EXCERPT_CONTEXT).max(1);
    text.lines()
        .enumerate()
        .map(|(i, l)| (i as u32 + 1, l))
        .skip_while(|(n, _)| *n < first)
        .take_while(|(n, _)| *n <= line + EXCERPT_CONTEXT)
        .map(|(n, l)| (n, l.replace('\t', "    ")))
        .collect()
}

/// The Problems panel.
pub fn problems_panel(ui: &mut egui::Ui, session: &mut EditorSession, tools: &mut GameTools) {
    ui.horizontal(|ui| {
        let n = tools.problems.len();
        if n == 0 {
            ui.label("No script problems.");
        } else {
            ui.label(RichText::new(format!("{n} script problem(s)")).strong());
        }
        if ui
            .small_button("Check scripts")
            .on_hover_text("Loads every script again and lists the ones that do not load")
            .clicked()
        {
            tools.refresh_schema(session);
        }
        if let Some(busy) = tools.busy_label() {
            ui.label(RichText::new(busy).small().weak());
        }
    });
    if !scripting_on(&session.project) {
        ui.label(
            RichText::new("Scripting is off for this project (Project settings > Scripting).")
                .small()
                .weak(),
        );
    }
    ui.separator();
    let problems: Vec<ScriptDiagnostic> = tools.problems.all().cloned().collect();
    let mut open = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, p) in problems.iter().enumerate() {
                ui.push_id(i, |ui| {
                    if problem_row(ui, session, p) {
                        open = Some(p.file.clone());
                    }
                });
                ui.add_space(4.0);
            }
        });
    if let Some(file) = open {
        open_in_editor(session, &file);
    }
}

/// One problem; true when Open was clicked.
fn problem_row(ui: &mut egui::Ui, session: &EditorSession, p: &ScriptDiagnostic) -> bool {
    let mut open = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let (tag, color) = if p.is_runtime() {
                ("Runtime error", Color32::from_rgb(235, 90, 80))
            } else {
                ("Does not load", Color32::from_rgb(232, 170, 80))
            };
            ui.colored_label(color, RichText::new(tag).strong());
            ui.label(RichText::new(p.location()).monospace());
            if ui
                .small_button("Open")
                .on_hover_text("Opens the script in your text editor")
                .clicked()
            {
                open = true;
            }
        });
        ui.label(&p.message);
        let mut context = Vec::new();
        if !p.kind.is_empty() {
            context.push(format!("behaviour {}", p.kind));
        }
        if let Some(e) = &p.entity {
            context.push(format!("entity \"{e}\""));
        }
        if let Some(c) = &p.callback {
            context.push(format!("in {c}"));
        }
        if let Some(t) = p.tick {
            context.push(format!("tick {t}"));
        }
        if p.is_runtime() {
            context.push("switched off for the rest of that run".into());
        }
        if !context.is_empty() {
            ui.label(RichText::new(context.join(" · ")).small().weak());
        }
        if let Some(line) = p.line {
            let lines = excerpt(session, &p.file, line);
            if !lines.is_empty() {
                egui::Frame::new()
                    .fill(theme::BAR)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        for (n, text) in lines {
                            let row = format!("{n:>4} │ {text}");
                            let rich = RichText::new(row).monospace();
                            if n == line {
                                ui.label(rich.color(theme::TEXT).strong());
                            } else {
                                ui.label(rich.weak());
                            }
                        }
                    });
            }
        }
    });
    open
}

/// The New script dialog's state.
#[derive(Debug, Clone, Default)]
pub struct NewScript {
    /// What the user typed (`door`, `puzzles/door`; C#: `DoorOpener`).
    pub name: String,
    /// A C# script rather than Lua.
    pub csharp: bool,
    /// Attach the new script to this entity.
    pub attach: Option<EntityId>,
    pub error: Option<String>,
    /// Folder under `scripts/` the script goes in (`puzzles`), or empty.
    /// The Assets panel sets it to the folder it has open.
    pub folder: String,
}

impl NewScript {
    /// Starts in C# when Lua is off.
    pub fn new(project: &Project, attach: Option<EntityId>) -> Self {
        Self {
            attach,
            csharp: !project.file.scripting.lua && project.file.scripting.csharp,
            ..Self::default()
        }
    }

    /// The file under `scripts/` the typed name becomes.
    fn path(&self) -> Result<String, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("type a name for the script".into());
        }
        let full = if self.folder.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.folder)
        };
        if self.csharp {
            new_cs_script_path(&full)
        } else {
            new_script_path(&full)
        }
    }

    fn kind(&self, rel: &str) -> String {
        if self.csharp {
            cs_kind_for(rel)
        } else {
            lua_kind_for(rel)
        }
    }
}

/// Creates the script; on success attaches it (as an undoable edit), opens
/// it and reads the behaviour kinds again.
fn create(
    session: &mut EditorSession,
    tools: &mut GameTools,
    dialog: &NewScript,
) -> Result<(), String> {
    let rel = dialog.path()?;
    let kind = dialog.kind(&rel);
    if dialog.csharp {
        create_cs_script(&session.project.root, &rel)?;
    } else {
        create_lua_script(&session.project.root, &rel)?;
    }
    let file = format!("{SCRIPTS_DIR}/{rel}");
    session.info(format!("created {file} (behaviour {kind})"));
    if let Some(entity) = dialog.attach.and_then(|id| session.scene.get(id).cloned())
        && entity.behavior(&kind).is_none()
    {
        let mut behaviors = entity.behaviors.clone();
        behaviors.push(BehaviorComponent::new(kind));
        session.execute(Command::SetBehaviors {
            id: entity.id,
            behaviors,
        });
    }
    open_in_editor(session, &file);
    tools.refresh_schema(session);
    Ok(())
}

/// Draws the New script dialog while it is open.
pub fn new_script_dialog(ctx: &egui::Context, session: &mut EditorSession, tools: &mut GameTools) {
    let Some(mut dialog) = tools.new_script.take() else {
        return;
    };
    let mut keep = true;
    egui::Modal::new(egui::Id::unique("copperframe-new-script")).show(ctx, |ui| {
        ui.heading("New script");
        ui.label(format!(
            "Creates a script in {SCRIPTS_DIR}/ from a template and opens it in your text editor."
        ));
        if !dialog.folder.is_empty() {
            ui.label(format!("In {SCRIPTS_DIR}/{}/", dialog.folder));
        }
        let langs = &session.project.file.scripting;
        if langs.lua && langs.csharp {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut dialog.csharp, false, "Lua");
                ui.selectable_value(&mut dialog.csharp, true, "C#");
            });
        }
        let hint = if dialog.csharp {
            "DoorOpener, or Puzzles/DoorOpener"
        } else {
            "door, or puzzles/door"
        };
        let field = ui.add(
            egui::TextEdit::singleline(&mut dialog.name)
                .hint_text(hint)
                .desired_width(260.0),
        );
        if !field.has_focus() && dialog.name.is_empty() {
            field.request_focus();
        }
        let checked = dialog.path();
        match &checked {
            Ok(rel) => {
                ui.label(
                    RichText::new(format!(
                        "{SCRIPTS_DIR}/{rel} · behaviour {}",
                        dialog.kind(rel)
                    ))
                    .small()
                    .weak(),
                );
            }
            Err(e) if !dialog.name.trim().is_empty() => {
                ui.colored_label(Color32::from_rgb(235, 90, 80), e);
            }
            Err(_) => {
                let rule = if dialog.csharp {
                    format!(
                        "A class name, like DoorOpener; folders with /. The behaviour kind is \
                         {CS_KIND_PREFIX}<name in snake_case>."
                    )
                } else {
                    format!(
                        "a-z, 0-9 and _; folders with /. The behaviour kind is {LUA_KIND_PREFIX}<name>."
                    )
                };
                ui.label(RichText::new(rule).small().weak());
            }
        }
        if let Some(name) = dialog
            .attach
            .and_then(|id| session.scene.get(id))
            .map(|e| e.name.clone())
        {
            ui.label(format!("It is added to \"{name}\"."));
        }
        if let Some(e) = &dialog.error {
            ui.colored_label(Color32::from_rgb(235, 90, 80), e);
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            let ok = ui.add_enabled(checked.is_ok(), egui::Button::new("Create"));
            if ok.clicked() || (enter && checked.is_ok()) {
                match create(session, tools, &dialog) {
                    Ok(()) => keep = false,
                    Err(e) => dialog.error = Some(e),
                }
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
        });
    });
    if keep {
        tools.new_script = Some(dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diag(file: &str, tick: Option<u64>) -> ScriptDiagnostic {
        ScriptDiagnostic {
            file: file.into(),
            line: Some(3),
            kind: "lua.door".into(),
            entity: tick.map(|_| "Door".into()),
            callback: tick.map(|_| "start".into()),
            message: "boom".into(),
            tick,
        }
    }

    #[test]
    fn new_scripts_go_in_the_chosen_folder() {
        let mut d = NewScript {
            folder: "puzzles".into(),
            ..NewScript::default()
        };
        assert!(d.path().is_err(), "a folder alone is not a script");
        d.name = "lever".into();
        assert_eq!(d.path().unwrap(), "puzzles/lever.lua");
        assert_eq!(d.kind("puzzles/lever.lua"), "lua.puzzles.lever");
        d.csharp = true;
        d.folder = "Puzzles".into();
        d.name = "LeverPull".into();
        assert_eq!(d.path().unwrap(), "Puzzles/LeverPull.cs");
        d.folder.clear();
        assert_eq!(d.path().unwrap(), "LeverPull.cs");
    }

    #[test]
    fn load_and_runtime_problems_are_kept_apart() {
        let mut p = Problems::default();
        let schema =
            json!({ "problems": [serde_json::to_value(diag("scripts/a.lua", None)).unwrap()] });
        assert_eq!(p.set_load(&schema), 1);
        assert!(std::mem::take(&mut p.arrived));
        // The game reports the same load problem again at the start of Play.
        p.add_runtime(diag("scripts/a.lua", None));
        assert!(!p.arrived);
        p.add_runtime(diag("scripts/b.lua", Some(4)));
        p.add_runtime(diag("scripts/b.lua", Some(4)));
        assert!(p.arrived);
        assert_eq!(p.len(), 2);
        let files: Vec<&str> = p.all().map(|d| d.file.as_str()).collect();
        assert_eq!(files, ["scripts/a.lua", "scripts/b.lua"]);
        p.clear_runtime();
        assert_eq!(p.len(), 1);
        p.arrived = false;
        assert_eq!(p.set_load(&json!({ "behaviors": [] })), 0);
        assert!(p.is_empty() && !p.arrived);
    }
}
