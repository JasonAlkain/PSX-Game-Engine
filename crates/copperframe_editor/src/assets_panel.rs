//! The asset browser: folders on the left, a grid of tiles on the right, and
//! a strip of details and actions for the selected asset. Models can be
//! dragged from here into the viewport.
//!
//! It shows the project's three folders: `assets` (models, textures,
//! sounds, and text or data files), `scenes` and `scripts` (Lua and C#).
//! Create (or a right-click) makes folders, scenes, scripts and data files
//! in the open folder; right-click a tile to open or delete it. Scripts and
//! data files open in the program the system uses for them. Deleting moves
//! things to `.psx/deleted/` in the project, and first lists the scenes
//! that use what is going.
//!
//! Scenes: double-click opens one, right-click renames, deletes or makes it
//! the start scene, and dragging a scene onto a folder moves it. File
//! operations go through [`EditorSession`] so references to the scene
//! follow it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use copperframe_assets::csharp::cs_kind_for;
use copperframe_assets::files::{self, DataFile, FileKind};
use copperframe_assets::project::{AudioSettings, ScriptingSettings};
use copperframe_assets::scripts::{SCRIPTS_DIR, lua_kind_for};
use copperframe_assets::{AssetEntry, AssetKind};
use copperframe_core::AssetId;
use copperframe_runtime::scene_gpu::{AssetStatus, SceneResources};
use egui::{Color32, RichText, Sense, TextureHandle, vec2};

use crate::audio_tools::EditorAudio;
use crate::icons::{self, Icon};
use crate::session::{DELETED_DIR, EditorSession, LogLevel, SCENE_SUFFIX, scene_stem};
use crate::theme;
use crate::ui::{Action, level_color};
use crate::viewport::EditorCamera;

const ROOT: &str = copperframe_assets::project::ASSETS_DIR;
const SCENES: &str = copperframe_assets::project::SCENES_DIR;
/// Largest thumbnail side, in pixels.
const THUMBNAIL: u32 = 128;
/// Files above this size show the plain icon instead of a thumbnail.
const THUMBNAIL_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Images with a longer side than this show the plain icon too.
const THUMBNAIL_MAX_SIDE: u32 = 8192;
/// Seconds between listings of the project's folders and plain files.
const RELIST: f64 = 1.0;

/// Which kinds of asset the grid lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Kind(AssetKind),
    /// Every scene in the project (a search looks through all of them).
    Scenes,
    /// Every Lua and C# script in the project.
    Scripts,
}

/// What a scene tile carries when dragged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SceneDrag(String);

/// Something only the editor's UI can do (it may ask about unsaved edits).
#[derive(Debug, Clone, PartialEq)]
pub enum PanelRequest {
    OpenScene(PathBuf),
    /// Show the New Scene dialog for this folder (relative to the project).
    NewScene(String),
    /// Show the New script dialog with the name starting with `prefix`
    /// (the folder under `scripts/`, ending in `/`, or empty).
    NewScript {
        prefix: String,
        csharp: bool,
    },
}

/// A dialog the panel shows over the editor.
#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    Rename {
        scene: String,
        name: String,
        error: Option<String>,
    },
    /// Moving a file or folder to the project's bin.
    Delete {
        rel: String,
        /// What it is ("scene", "Lua script", "folder").
        what: String,
        /// What else goes or breaks with it.
        notes: Vec<String>,
        error: Option<String>,
    },
    NewFolder {
        parent: String,
        name: String,
        error: Option<String>,
    },
    NewDataFile {
        folder: String,
        kind: DataFile,
        name: String,
        error: Option<String>,
    },
}

/// The panel's state. Thumbnails are per asset, so they are dropped when
/// assets change or another scene opens.
pub struct AssetsPanel {
    folder: String,
    search: String,
    filter: Filter,
    tile: f32,
    selected: Option<AssetId>,
    /// Selected scene file, relative to the project.
    selected_scene: Option<String>,
    /// Selected script or other plain file, relative to the project.
    selected_file: Option<String>,
    /// Every folder of the three roots, and the plain files
    /// ([`files::plain_files`]), as last listed.
    folders: Vec<String>,
    files: Vec<String>,
    /// When they were listed (egui time); `None` lists them again.
    listed_at: Option<f64>,
    thumbnails: HashMap<AssetId, Option<TextureHandle>>,
    dialog: Option<Dialog>,
    /// Requests for the editor, taken by the UI each frame.
    pub requests: Vec<PanelRequest>,
}

impl Default for AssetsPanel {
    fn default() -> Self {
        Self {
            folder: ROOT.to_owned(),
            search: String::new(),
            filter: Filter::All,
            tile: 72.0,
            selected: None,
            selected_scene: None,
            selected_file: None,
            folders: Vec::new(),
            files: Vec::new(),
            listed_at: None,
            thumbnails: HashMap::new(),
            dialog: None,
            requests: Vec::new(),
        }
    }
}

/// The folder of a path relative to the project root.
fn folder_of(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn file_name(relative: &str) -> &str {
    relative.rsplit_once('/').map_or(relative, |(_, name)| name)
}

/// Direct subfolders of `folder`.
fn subfolders<'a>(all: &'a [String], folder: &str) -> Vec<&'a str> {
    all.iter()
        .map(String::as_str)
        .filter(|f| folder_of(f) == folder)
        .collect()
}

/// Whether the grid lists `relative`. A search looks through every folder
/// below the open one; without a search only the open folder is listed.
fn listed(relative: &str, folder: &str, search: &str) -> bool {
    if search.is_empty() {
        return folder_of(relative) == folder;
    }
    let inside = relative
        .strip_prefix(folder)
        .is_some_and(|rest| rest.starts_with('/'));
    inside
        && file_name(relative)
            .to_lowercase()
            .contains(&search.to_lowercase())
}

/// Whether `folder` is `scenes` or inside it.
fn in_scenes(folder: &str) -> bool {
    folder == SCENES || folder.starts_with(&format!("{SCENES}/"))
}

/// A scene's name as shown: the file name without `.scene.json`.
fn scene_label(relative: &str) -> &str {
    let name = file_name(relative);
    name.strip_suffix(SCENE_SUFFIX).unwrap_or(name)
}

/// Whether the grid lists the scene `relative` (see [`listed`]); with the
/// Scenes filter every folder is searched.
fn scene_listed(relative: &str, folder: &str, search: &str, filter: Filter) -> bool {
    match filter {
        Filter::Kind(_) | Filter::Scripts => false,
        Filter::Scenes => {
            search.is_empty()
                || scene_label(relative)
                    .to_lowercase()
                    .contains(&search.to_lowercase())
        }
        Filter::All => listed(relative, folder, search),
    }
}

/// Whether the grid lists the plain file `relative` (see [`listed`]); with
/// the Scripts filter every script is searched.
fn file_listed(relative: &str, folder: &str, search: &str, filter: Filter) -> bool {
    match filter {
        Filter::Kind(_) | Filter::Scenes => false,
        Filter::Scripts => {
            FileKind::of(relative).is_script()
                && (search.is_empty()
                    || file_name(relative)
                        .to_lowercase()
                        .contains(&search.to_lowercase()))
        }
        Filter::All => listed(relative, folder, search),
    }
}

/// The behaviour kind a script file becomes, if it is one.
fn script_kind(relative: &str) -> Option<String> {
    let inner = relative.strip_prefix(&format!("{SCRIPTS_DIR}/"))?;
    match FileKind::of(relative) {
        FileKind::Lua => Some(lua_kind_for(inner)),
        FileKind::CSharp => Some(cs_kind_for(inner)),
        FileKind::Other => None,
    }
}

fn file_icon(kind: FileKind) -> (Icon, Color32) {
    match kind {
        FileKind::Lua => (Icon::Script, Color32::from_rgb(120, 150, 230)),
        FileKind::CSharp => (Icon::Script, Color32::from_rgb(160, 200, 110)),
        FileKind::Other => (Icon::File, theme::TEXT_DIM),
    }
}

/// "2 scenes (scenes/a.scene.json ×2, scenes/b.scene.json)".
fn describe_uses(uses: &[(String, usize)]) -> String {
    let list: Vec<String> = uses
        .iter()
        .map(|(scene, n)| {
            if *n > 1 {
                format!("{scene} ×{n}")
            } else {
                scene.clone()
            }
        })
        .collect();
    let scenes = if uses.len() == 1 { "scene" } else { "scenes" };
    format!("{} {scenes} ({})", uses.len(), list.join(", "))
}

/// Files inside a folder, at any depth.
fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}

fn kind_icon(kind: AssetKind) -> Icon {
    match kind {
        AssetKind::Gltf => Icon::Model,
        AssetKind::Texture => Icon::Texture,
        AssetKind::Audio => Icon::Sound,
    }
}

fn kind_color(kind: AssetKind) -> Color32 {
    match kind {
        AssetKind::Gltf => theme::ACCENT,
        AssetKind::Texture => Color32::from_rgb(112, 178, 170),
        AssetKind::Audio => Color32::from_rgb(190, 160, 220),
    }
}

/// The status colour and words for an asset, if it has one to show.
fn status_of(entry: &AssetEntry, resources: &SceneResources) -> Option<(Color32, String)> {
    // Sounds are loaded by the game, not the editor.
    if entry.kind == AssetKind::Audio {
        return None;
    }
    match resources.status(entry.id) {
        None => None,
        Some(AssetStatus::Loaded { warnings }) if warnings.is_empty() => None,
        Some(AssetStatus::Loaded { warnings }) => Some((
            level_color(LogLevel::Warning),
            format!("{} warnings:\n{}", warnings.len(), warnings.join("\n")),
        )),
        Some(AssetStatus::Failed { error, .. }) => Some((
            level_color(LogLevel::Error),
            format!("import failed: {error}"),
        )),
        Some(AssetStatus::Missing) => Some((
            level_color(LogLevel::Error),
            "missing from the project".to_owned(),
        )),
    }
}

fn load_thumbnail(ctx: &egui::Context, entry: &AssetEntry) -> Option<TextureHandle> {
    if std::fs::metadata(&entry.source).ok()?.len() > THUMBNAIL_MAX_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(&entry.source)
        .ok()?
        .with_guessed_format()
        .ok()?;
    // A small file can still claim to be enormous once decoded.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(THUMBNAIL_MAX_SIDE);
    limits.max_image_height = Some(THUMBNAIL_MAX_SIDE);
    limits.max_alloc = Some(THUMBNAIL_MAX_BYTES * 16);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if image.width() > THUMBNAIL || image.height() > THUMBNAIL {
        image.thumbnail(THUMBNAIL, THUMBNAIL)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(
        format!("asset-thumbnail-{}", entry.id),
        pixels,
        egui::TextureOptions::NEAREST,
    ))
}

impl AssetsPanel {
    /// Forgets thumbnails so they are read again (and releases their memory).
    pub fn clear_thumbnails(&mut self) {
        self.thumbnails.clear();
    }

    /// Lists folders and plain files again on the next frame.
    pub fn relist(&mut self) {
        self.listed_at = None;
    }

    /// `audio` previews sounds through the project's `mixer` settings.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        actions: &mut Vec<Action>,
        audio: &mut EditorAudio,
        mixer: AudioSettings,
    ) {
        let entries = session.db.entries.clone();
        let scenes: Vec<String> = session
            .scene_files()
            .iter()
            .map(|p| session.project.relative(p))
            .collect();
        let now = ui.input(|i| i.time);
        if self.listed_at.is_none_or(|t| now - t > RELIST || now < t) {
            self.folders = files::folders(&session.project.root);
            self.files = files::plain_files(&session.project.root);
            self.listed_at = Some(now);
        }
        let all = self.folders.clone();
        if !all.contains(&self.folder) {
            self.folder = ROOT.to_owned();
        }
        let langs = session.project.file.scripting;
        self.top_bar(ui, session, actions, langs);
        ui.separator();
        // The details strip takes its height first so the grid can fill what
        // is left.
        egui::Panel::bottom("asset-details")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 3)))
            .show(ui, |ui| {
                self.details(
                    ui, session, camera, resources, &entries, actions, audio, mixer,
                );
            });
        egui::Panel::left("asset-folders")
            .resizable(true)
            .default_size(150.0)
            .min_size(70.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.folder_tree(ui, session, &all, langs));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.breadcrumb(ui);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.grid(
                        ui, session, camera, resources, &entries, &all, &scenes, audio, mixer,
                    );
                });
        });
        self.dialogs(ui.ctx(), session, actions, audio);
    }

    fn top_bar(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        actions: &mut Vec<Action>,
        langs: ScriptingSettings,
    ) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search this folder…")
                    .desired_width(170.0),
            );
            for (filter, label) in [
                (Filter::All, "All"),
                (Filter::Kind(AssetKind::Gltf), "Models"),
                (Filter::Kind(AssetKind::Texture), "Textures"),
                (Filter::Kind(AssetKind::Audio), "Sounds"),
                (Filter::Scenes, "Scenes"),
                (Filter::Scripts, "Scripts"),
            ] {
                ui.selectable_value(&mut self.filter, filter, label);
            }
            ui.menu_button("Create", |ui| self.create_menu(ui, langs));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Slider::new(&mut self.tile, 48.0..=128.0)
                        .show_value(false)
                        .trailing_fill(true),
                )
                .on_hover_text("Tile size");
                if ui
                    .button("Rescan")
                    .on_hover_text(
                        "The project's folders are watched: new and changed files appear \
                         and are imported automatically within a second or two",
                    )
                    .clicked()
                {
                    session.rescan_assets();
                    self.clear_thumbnails();
                    self.relist();
                    actions.push(Action::AssetsChanged);
                }
            });
        });
    }

    fn folder_tree(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        all: &[String],
        langs: ScriptingSettings,
    ) {
        for folder in all {
            let depth = folder.matches('/').count();
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                let rect = icons::row_rect(ui, 13.0);
                let icon = match folder.as_str() {
                    SCENES => Icon::Scene,
                    SCRIPTS_DIR => Icon::Script,
                    _ => Icon::Folder,
                };
                icons::paint(ui.painter(), rect, icon, theme::TEXT_DIM);
                let open = self.folder == *folder;
                let response = ui.selectable_label(open, file_name(folder));
                if response.clicked() || response.secondary_clicked() {
                    self.folder = folder.clone();
                }
                if in_scenes(folder) {
                    self.scene_drop_target(ui, session, &response, folder);
                }
                response.context_menu(|ui| self.folder_menu(ui, session, folder, langs));
            });
        }
    }

    /// Right-click menu of a folder (in the tree or the grid): what can be
    /// made in it, and deleting it.
    fn folder_menu(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        folder: &str,
        langs: ScriptingSettings,
    ) {
        if self.folder != folder {
            if ui.button("Open").clicked() {
                self.folder = folder.to_owned();
                ui.close();
            }
            ui.separator();
        }
        // The create menu works on the open folder.
        let open = std::mem::replace(&mut self.folder, folder.to_owned());
        self.create_menu(ui, langs);
        self.folder = open;
        ui.separator();
        if ui.button("Show in folder").clicked() {
            open_folder(session, &session.project.resolve(folder));
            ui.close();
        }
        if !files::ROOTS.contains(&folder) && ui.button("Delete…").clicked() {
            self.ask_delete(session, folder);
            ui.close();
        }
    }

    /// The folder in `root` new things of that root go in: the open folder
    /// if it is inside `root`, otherwise `root` itself.
    fn target_in(&self, root: &str) -> String {
        if files::root_of(&self.folder) == Some(root) || self.folder == root {
            self.folder.clone()
        } else {
            root.to_owned()
        }
    }

    /// Everything that can be made, each in the open folder when it is the
    /// right kind of folder, else at the top of its own.
    fn create_menu(&mut self, ui: &mut egui::Ui, langs: ScriptingSettings) {
        let here = self.folder.clone();
        if ui
            .button("Folder…")
            .on_hover_text(format!("A new folder in {here}/"))
            .clicked()
        {
            self.dialog = Some(Dialog::NewFolder {
                parent: here,
                name: "New folder".into(),
                error: None,
            });
            ui.close();
        }
        ui.separator();
        let scenes = self.target_in(SCENES);
        if ui
            .button("Scene…")
            .on_hover_text(format!("A new scene in {scenes}/"))
            .clicked()
        {
            self.requests.push(PanelRequest::NewScene(scenes));
            ui.close();
        }
        let scripts = self.target_in(SCRIPTS_DIR);
        let prefix = scripts
            .strip_prefix(SCRIPTS_DIR)
            .unwrap_or("")
            .trim_start_matches('/');
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        for (csharp, label, on) in [
            (false, "Lua script…", langs.lua),
            (true, "C# script…", langs.csharp),
        ] {
            if ui
                .add_enabled(on, egui::Button::new(label))
                .on_hover_text(format!(
                    "A new script in {scripts}/ from a template, opened in your text editor"
                ))
                .on_disabled_hover_text("Turn the language on in Project settings > Scripting")
                .clicked()
            {
                self.requests.push(PanelRequest::NewScript {
                    prefix: prefix.clone(),
                    csharp,
                });
                ui.close();
            }
        }
        ui.separator();
        let data = self.target_in(ROOT);
        for kind in DataFile::ALL {
            if ui
                .button(format!("{}…", kind.label()))
                .on_hover_text(format!(
                    "A new .{} file in {data}/; it is copied into exported games",
                    kind.extension()
                ))
                .clicked()
            {
                self.dialog = Some(Dialog::NewDataFile {
                    folder: data.clone(),
                    kind,
                    name: String::new(),
                    error: None,
                });
                ui.close();
            }
        }
    }

    /// Opens the Delete dialog for `rel`, listing what else is affected.
    fn ask_delete(&mut self, session: &EditorSession, rel: &str) {
        let root = &session.project.root;
        let path = session.project.resolve(rel);
        let mut notes = Vec::new();
        let what = if path.is_dir() {
            let n = count_files(&path);
            notes.push(match n {
                0 => "The folder is empty.".to_owned(),
                1 => "The 1 file inside goes with it.".to_owned(),
                n => format!("The {n} files inside go with it."),
            });
            if files::root_of(rel) == Some(ROOT) && n > 0 {
                notes.push(
                    "Entities using models, textures or sounds from it will show them as \
                     missing."
                        .into(),
                );
            }
            if files::root_of(rel) == Some(SCRIPTS_DIR) && n > 0 {
                notes.push(
                    "Scenes using its scripts will not play until the scripts are back or \
                     their components are removed."
                        .into(),
                );
            }
            "folder".to_owned()
        } else if rel.ends_with(SCENE_SUFFIX) {
            let uses: Vec<_> = files::scenes_mentioning(root, rel)
                .into_iter()
                .filter(|(s, _)| s != rel)
                .collect();
            if !uses.is_empty() {
                notes.push(format!(
                    "Behaviour settings name it in {}; they will point at a missing scene.",
                    describe_uses(&uses)
                ));
            }
            "scene".to_owned()
        } else if let Some(entry) = session.db.entries.iter().find(|e| e.relative == rel) {
            let uses = files::scenes_mentioning(root, &entry.id.to_string());
            if uses.is_empty() {
                notes.push("No scene uses it.".into());
            } else {
                notes.push(format!(
                    "Used in {}; those entities will show it as missing.",
                    describe_uses(&uses)
                ));
            }
            notes.push(
                "Its .meta.json goes with it, so moving both back restores it with the same ID."
                    .into(),
            );
            entry.kind.label().to_owned()
        } else {
            let kind = FileKind::of(rel);
            if let Some(behaviour) = script_kind(rel) {
                let uses = files::scenes_mentioning(root, &behaviour);
                if uses.is_empty() {
                    notes.push(format!("No scene uses its behaviour {behaviour}."));
                } else {
                    notes.push(format!(
                        "Its behaviour {behaviour} is used in {}; those scenes will not play \
                         until the script is back or the components are removed.",
                        describe_uses(&uses)
                    ));
                }
            }
            kind.label().to_owned()
        };
        self.dialog = Some(Dialog::Delete {
            rel: rel.to_owned(),
            what,
            notes,
            error: None,
        });
    }

    /// Lets a dragged scene be dropped onto a scene folder (moving it).
    fn scene_drop_target(
        &mut self,
        ui: &egui::Ui,
        session: &mut EditorSession,
        response: &egui::Response,
        folder: &str,
    ) {
        if response.dnd_hover_payload::<SceneDrag>().is_some() {
            ui.painter().rect_stroke(
                response.rect,
                3.0,
                egui::Stroke::new(1.5, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(drag) = response.dnd_release_payload::<SceneDrag>() {
            let rel = &drag.0;
            if folder_of(rel) != folder {
                let from = session.project.resolve(rel);
                let to = session.project.resolve(folder).join(file_name(rel));
                match session.move_scene(&from, &to) {
                    Ok(_) => self.selected_scene = Some(session.project.relative(&to)),
                    Err(e) => session.error(e),
                }
            }
        }
    }

    fn breadcrumb(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let path = self.folder.clone();
            let mut upto = String::new();
            for (i, part) in path.split('/').enumerate() {
                if i > 0 {
                    upto.push('/');
                    ui.label(RichText::new("/").weak());
                }
                upto.push_str(part);
                if ui.link(part).clicked() {
                    self.folder = upto.clone();
                }
            }
        });
    }

    fn select(&mut self, asset: Option<AssetId>, scene: Option<&str>, file: Option<&str>) {
        self.selected = asset;
        self.selected_scene = scene.map(str::to_owned);
        self.selected_file = file.map(str::to_owned);
    }

    #[allow(clippy::too_many_arguments)]
    fn grid(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        entries: &[AssetEntry],
        all: &[String],
        scenes: &[String],
        audio: &mut EditorAudio,
        mixer: AudioSettings,
    ) {
        let mut any = false;
        let mut loaded_one = false;
        let search = self.search.trim().to_owned();
        let langs = session.project.file.scripting;
        let plain = self.files.clone();
        let response = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if search.is_empty() && matches!(self.filter, Filter::All | Filter::Kind(_)) {
                for sub in subfolders(all, &self.folder) {
                    any = true;
                    let (response, icon_rect) = self.tile(ui, file_name(sub));
                    icons::paint(ui.painter(), icon_rect, Icon::Folder, theme::TEXT_DIM);
                    let badge = match sub {
                        SCENES => Some(Icon::Scene),
                        SCRIPTS_DIR => Some(Icon::Script),
                        _ => None,
                    };
                    if let Some(badge) = badge {
                        icons::paint(
                            ui.painter(),
                            icon_rect
                                .shrink(icon_rect.width() * 0.3)
                                .translate(vec2(0.0, 3.0)),
                            badge,
                            theme::TEXT,
                        );
                    }
                    if response.double_clicked() {
                        self.folder = sub.to_owned();
                    }
                    if in_scenes(sub) {
                        self.scene_drop_target(ui, session, &response, sub);
                    }
                    response
                        .on_hover_text(format!("{sub}/\nDouble-click to open"))
                        .context_menu(|ui| self.folder_menu(ui, session, sub, langs));
                }
            }
            for rel in scenes {
                if !scene_listed(rel, &self.folder, &search, self.filter) {
                    continue;
                }
                any = true;
                self.scene_tile(ui, session, rel);
            }
            for rel in &plain {
                if !file_listed(rel, &self.folder, &search, self.filter) {
                    continue;
                }
                any = true;
                self.file_tile(ui, session, rel);
            }
            if matches!(self.filter, Filter::Scenes | Filter::Scripts) {
                return;
            }
            for entry in entries {
                if !listed(&entry.relative, &self.folder, self.search.trim()) {
                    continue;
                }
                if let Filter::Kind(kind) = self.filter
                    && entry.kind != kind
                {
                    continue;
                }
                any = true;
                let (response, icon_rect) = self.tile(ui, file_name(&entry.relative));
                let selected = self.selected == Some(entry.id);
                if selected {
                    ui.painter().rect_stroke(
                        response.rect,
                        3.0,
                        egui::Stroke::new(1.5, theme::ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                // One image is decoded per frame so the panel stays responsive.
                if entry.kind == AssetKind::Texture
                    && !self.thumbnails.contains_key(&entry.id)
                    && !loaded_one
                {
                    loaded_one = true;
                    let thumbnail = load_thumbnail(ui.ctx(), entry);
                    self.thumbnails.insert(entry.id, thumbnail);
                    ui.ctx().request_repaint();
                }
                match self.thumbnails.get(&entry.id) {
                    Some(Some(texture)) => {
                        let fit = egui::Image::new(texture).fit_to_exact_size(icon_rect.size());
                        fit.paint_at(ui, icon_rect);
                    }
                    _ => icons::paint(
                        ui.painter(),
                        icon_rect,
                        kind_icon(entry.kind),
                        kind_color(entry.kind),
                    ),
                }
                if let Some((color, _)) = status_of(entry, resources) {
                    ui.painter().circle_filled(
                        response.rect.right_top() + vec2(-8.0, 8.0),
                        4.0,
                        color,
                    );
                }
                if response.clicked() || response.drag_started() || response.secondary_clicked() {
                    self.select(Some(entry.id), None, None);
                }
                if entry.kind == AssetKind::Gltf {
                    response.dnd_set_drag_payload(entry.id);
                    if response.double_clicked() {
                        session.add_model(entry.id, camera.spawn_point(4.0));
                    }
                }
                let mut hover = format!("{}\n{}", entry.relative, entry.kind.label());
                if entry.kind == AssetKind::Audio {
                    let playing = audio.state().is_some_and(|s| s.asset == entry.id);
                    if playing {
                        // A small play mark in the corner while it sounds.
                        let mark = egui::Rect::from_center_size(
                            response.rect.left_top() + vec2(10.0, 10.0),
                            vec2(10.0, 10.0),
                        );
                        icons::paint(ui.painter(), mark, Icon::Play, theme::ACCENT);
                    }
                    if response.double_clicked() {
                        audio.toggle(session, entry, mixer);
                    }
                    hover.push_str(if playing {
                        "\nDouble-click to stop"
                    } else {
                        "\nDouble-click to preview"
                    });
                }
                response
                    .on_hover_text(hover)
                    .context_menu(|ui| self.asset_menu(ui, session, camera, entry, langs));
            }
        });
        let rest = ui.allocate_response(ui.available_size().max(vec2(1.0, 40.0)), Sense::click());
        let menu_target = response.response.union(rest);
        menu_target.context_menu(|ui| self.create_menu(ui, langs));
        if !any {
            let text = match self.filter {
                Filter::Scenes => "No scenes. Create one with Create > Scene… or Ctrl+N.",
                Filter::Scripts => {
                    "No scripts. Create one with Create > Lua script… or C# script…."
                }
                _ if in_scenes(&self.folder) => {
                    "No scenes here. Create one with Create > Scene… or Ctrl+N."
                }
                _ if files::root_of(&self.folder) == Some(SCRIPTS_DIR) => {
                    "No scripts here. Create one with Create, or right-click."
                }
                _ => "Nothing here. Files you add to assets/ appear on their own.",
            };
            ui.label(RichText::new(text).weak());
        }
    }

    /// Right-click menu of an asset tile.
    fn asset_menu(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        entry: &AssetEntry,
        langs: ScriptingSettings,
    ) {
        if entry.kind == AssetKind::Gltf && ui.button("Add to scene").clicked() {
            session.add_model(entry.id, camera.spawn_point(4.0));
            ui.close();
        }
        if ui
            .button("Open")
            .on_hover_text("Opens the file in the program your system uses for it")
            .clicked()
        {
            open_file(session, &entry.relative);
            ui.close();
        }
        if ui.button("Show in folder").clicked() {
            if let Some(dir) = entry.source.parent() {
                open_folder(session, dir);
            }
            ui.close();
        }
        if ui.button("Delete…").clicked() {
            self.ask_delete(session, &entry.relative);
            ui.close();
        }
        ui.separator();
        self.create_menu(ui, langs);
    }

    /// A script or other plain file.
    fn file_tile(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, rel: &str) {
        let kind = FileKind::of(rel);
        let (response, icon_rect) = self.tile(ui, file_name(rel));
        let (icon, color) = file_icon(kind);
        icons::paint(ui.painter(), icon_rect.shrink(4.0), icon, color);
        if self.selected_file.as_deref() == Some(rel) {
            ui.painter().rect_stroke(
                response.rect,
                3.0,
                egui::Stroke::new(1.5, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            self.select(None, None, Some(rel));
        }
        if response.double_clicked() {
            open_file(session, rel);
        }
        let mut hover = format!("{rel}\n{}", kind.label());
        if let Some(behaviour) = script_kind(rel) {
            hover.push_str(&format!(" (behaviour {behaviour})"));
            let langs = session.project.file.scripting;
            let on = match kind {
                FileKind::Lua => langs.lua,
                _ => langs.csharp,
            };
            if !on {
                hover.push_str("\nIts language is off in Project settings > Scripting");
            }
        }
        hover.push_str("\nDouble-click to open");
        let langs = session.project.file.scripting;
        response.on_hover_text(hover).context_menu(|ui| {
            if ui.button("Open").clicked() {
                open_file(session, rel);
                ui.close();
            }
            if ui.button("Show in folder").clicked() {
                if let Some(dir) = session.project.resolve(rel).parent() {
                    open_folder(session, dir);
                }
                ui.close();
            }
            if ui.button("Delete…").clicked() {
                self.ask_delete(session, rel);
                ui.close();
            }
            ui.separator();
            self.create_menu(ui, langs);
        });
    }

    fn scene_tile(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, rel: &str) {
        let (response, icon_rect) = self.tile(ui, scene_label(rel));
        let path = session.project.resolve(rel);
        let open = path == session.scene_path;
        let startup = path == session.project.startup_scene_path();
        let color = if open { theme::ACCENT } else { theme::TEXT };
        icons::paint(ui.painter(), icon_rect.shrink(4.0), Icon::Scene, color);
        if self.selected_scene.as_deref() == Some(rel) {
            ui.painter().rect_stroke(
                response.rect,
                3.0,
                egui::Stroke::new(1.5, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if startup {
            ui.painter().text(
                response.rect.left_top() + vec2(5.0, 3.0),
                egui::Align2::LEFT_TOP,
                "start",
                egui::TextStyle::Small.resolve(ui.style()),
                theme::TEXT_DIM,
            );
        }
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            self.select(None, Some(rel), None);
        }
        response.dnd_set_drag_payload(SceneDrag(rel.to_owned()));
        if response.double_clicked() && !open {
            self.requests.push(PanelRequest::OpenScene(path.clone()));
        }
        let mut hover = format!("{rel}\nScene");
        if open {
            hover.push_str(" (open)");
        }
        if startup {
            hover.push_str(" (start scene)");
        }
        response
            .on_hover_text(hover)
            .context_menu(|ui| self.scene_menu(ui, session, rel));
    }

    fn scene_menu(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, rel: &str) {
        let path = session.project.resolve(rel);
        let open = path == session.scene_path;
        if ui.add_enabled(!open, egui::Button::new("Open")).clicked() {
            self.requests.push(PanelRequest::OpenScene(path.clone()));
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            self.dialog = Some(Dialog::Rename {
                scene: rel.to_owned(),
                name: scene_label(rel).to_owned(),
                error: None,
            });
            ui.close();
        }
        if ui.button("Delete…").clicked() {
            self.ask_delete(session, rel);
            ui.close();
        }
        let startup = path == session.project.startup_scene_path();
        if ui
            .add_enabled(!startup, egui::Button::new("Set as Start Scene"))
            .clicked()
        {
            let previous =
                std::mem::replace(&mut session.project.file.startup_scene, rel.to_owned());
            match session.project.save() {
                Ok(()) => session.info(format!("the game now starts with {rel}")),
                Err(e) => {
                    session.project.file.startup_scene = previous;
                    session.error(e.to_string());
                }
            }
            ui.close();
        }
        if ui.button("Show in folder").clicked() {
            if let Some(dir) = path.parent() {
                open_folder(session, dir);
            }
            ui.close();
        }
        ui.separator();
        let langs = session.project.file.scripting;
        self.create_menu(ui, langs);
    }

    fn dialogs(
        &mut self,
        ctx: &egui::Context,
        session: &mut EditorSession,
        actions: &mut Vec<Action>,
        audio: &mut EditorAudio,
    ) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let mut selected = None;
        let mut opened_folder = None;
        let mut changed = false;
        let mut deleted_assets = false;
        egui::Modal::new(egui::Id::unique("copperframe-scene-file-dialog")).show(ctx, |ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            match dialog {
                Dialog::Rename { scene, name, error } => {
                    ui.heading("Rename scene");
                    ui.label(scene.as_str());
                    ui.text_edit_singleline(name).request_focus();
                    let target = format!("{}{SCENE_SUFFIX}", scene_stem(name));
                    ui.label(RichText::new(format!("File: {target}")).weak());
                    let mut go = false;
                    ui.horizontal(|ui| {
                        go = ui.button("Rename").clicked() || enter;
                        close = ui.button("Cancel").clicked() || escape;
                    });
                    if go {
                        let from = session.project.resolve(scene);
                        let to = from.with_file_name(&target);
                        if to == from {
                            close = true;
                        } else if scene_stem(name).is_empty() {
                            *error = Some("the scene needs a name".into());
                        } else {
                            match session.move_scene(&from, &to) {
                                Ok(_) => {
                                    selected = Some(session.project.relative(&to));
                                    close = true;
                                }
                                Err(e) => *error = Some(e),
                            }
                        }
                    }
                    if let Some(e) = error {
                        ui.colored_label(level_color(LogLevel::Error), e.as_str());
                    }
                }
                Dialog::Delete {
                    rel,
                    what,
                    notes,
                    error,
                } => {
                    ui.set_max_width(460.0);
                    ui.heading(format!("Delete {what}"));
                    ui.label(format!(
                        "Delete {rel}? It is moved to {DELETED_DIR}/ in the project folder, \
                         so it can be recovered by hand."
                    ));
                    for note in notes.iter() {
                        ui.label(RichText::new(note.as_str()).weak());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            match session.delete_path(rel) {
                                Ok(()) => {
                                    deleted_assets = files::root_of(rel) == Some(ROOT);
                                    changed = true;
                                    close = true;
                                }
                                Err(e) => *error = Some(e),
                            }
                        }
                        close |= ui.button("Cancel").clicked() || escape;
                    });
                    if let Some(e) = error {
                        ui.colored_label(level_color(LogLevel::Error), e.as_str());
                    }
                }
                Dialog::NewFolder {
                    parent,
                    name,
                    error,
                } => {
                    ui.heading("New folder");
                    ui.label(format!("In {parent}/"));
                    ui.text_edit_singleline(name).request_focus();
                    let mut go = false;
                    ui.horizontal(|ui| {
                        go = ui.button("Create").clicked() || enter;
                        close = ui.button("Cancel").clicked() || escape;
                    });
                    if go {
                        match session.create_folder(parent, name) {
                            Ok(p) => {
                                opened_folder = Some(session.project.relative(&p));
                                changed = true;
                                close = true;
                            }
                            Err(e) => *error = Some(e),
                        }
                    }
                    if let Some(e) = error {
                        ui.colored_label(level_color(LogLevel::Error), e.as_str());
                    }
                }
                Dialog::NewDataFile {
                    folder,
                    kind,
                    name,
                    error,
                } => {
                    ui.heading(format!("New {}", kind.label().to_lowercase()));
                    ui.label(format!(
                        "In {folder}/. It opens in your text editor and is copied into \
                         exported games."
                    ));
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text(format!("notes, or notes.{}", kind.extension()))
                            .desired_width(260.0),
                    )
                    .request_focus();
                    let checked = files::data_file_name(name, *kind);
                    match &checked {
                        Ok(file) => {
                            ui.label(RichText::new(format!("{folder}/{file}")).small().weak());
                        }
                        Err(e) if !name.trim().is_empty() => {
                            ui.colored_label(level_color(LogLevel::Error), e.as_str());
                        }
                        Err(_) => {}
                    }
                    let mut go = false;
                    ui.horizontal(|ui| {
                        let ok = ui.add_enabled(checked.is_ok(), egui::Button::new("Create"));
                        go = ok.clicked() || (enter && checked.is_ok());
                        close = ui.button("Cancel").clicked() || escape;
                    });
                    if go {
                        match files::create_data_file(&session.project.root, folder, name, *kind) {
                            Ok(p) => {
                                let rel = session.project.relative(&p);
                                session.info(format!("created {rel}"));
                                open_file(session, &rel);
                                selected = Some(rel);
                                changed = true;
                                close = true;
                            }
                            Err(e) => *error = Some(e),
                        }
                    }
                    if let Some(e) = error {
                        ui.colored_label(level_color(LogLevel::Error), e.as_str());
                    }
                }
            }
        });
        if let Some(s) = selected {
            if s.ends_with(SCENE_SUFFIX) {
                self.select(None, Some(&s), None);
            } else {
                self.select(None, None, Some(&s));
            }
        }
        if let Some(f) = opened_folder {
            self.folder = f;
        }
        if changed {
            self.relist();
        }
        if deleted_assets {
            self.clear_thumbnails();
            audio.forget_missing(session);
            actions.push(Action::AssetsChanged);
        }
        if close {
            self.dialog = None;
        }
    }

    /// Lays out one tile; returns its response and where its picture goes.
    fn tile(&self, ui: &mut egui::Ui, name: &str) -> (egui::Response, egui::Rect) {
        let side = self.tile;
        let (rect, response) =
            ui.allocate_exact_size(vec2(side + 12.0, side + 24.0), Sense::click_and_drag());
        let fill = if response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            theme::WELL
        };
        ui.painter().rect_filled(rect, 3.0, fill);
        let picture = egui::Rect::from_center_size(
            rect.center_top() + vec2(0.0, 6.0 + side * 0.5),
            egui::Vec2::splat(side * 0.8),
        );
        let label = rect.center_bottom() + vec2(0.0, -10.0);
        let galley = ui.painter().layout(
            elide(name, side + 8.0),
            egui::TextStyle::Small.resolve(ui.style()),
            theme::TEXT,
            f32::INFINITY,
        );
        ui.painter()
            .galley(label - galley.size() * 0.5, galley, theme::TEXT);
        (response, picture)
    }

    #[allow(clippy::too_many_arguments)]
    fn details(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        entries: &[AssetEntry],
        actions: &mut Vec<Action>,
        audio: &mut EditorAudio,
        mixer: AudioSettings,
    ) {
        let entry = self
            .selected
            .and_then(|id| entries.iter().find(|e| e.id == id));
        if entry.is_none()
            && let Some(rel) = self.selected_file.clone()
        {
            if session.project.resolve(&rel).is_file() {
                self.file_details(ui, session, &rel);
                return;
            }
            self.selected_file = None;
        }
        if entry.is_none()
            && let Some(rel) = self.selected_scene.clone()
        {
            let path = session.project.resolve(&rel);
            if path.is_file() {
                ui.horizontal(|ui| {
                    let rect = icons::row_rect(ui, 14.0);
                    icons::paint(ui.painter(), rect, Icon::Scene, theme::TEXT);
                    ui.label(&rel);
                    if path == session.scene_path {
                        ui.label(RichText::new("open").color(theme::ACCENT));
                    } else if ui.button("Open").clicked() {
                        self.requests.push(PanelRequest::OpenScene(path.clone()));
                    }
                    if path == session.project.startup_scene_path() {
                        ui.label(RichText::new("start scene").weak());
                    }
                    if ui.button("Rename…").clicked() {
                        self.dialog = Some(Dialog::Rename {
                            scene: rel.clone(),
                            name: scene_label(&rel).to_owned(),
                            error: None,
                        });
                    }
                    if ui.button("Show in folder").clicked()
                        && let Some(dir) = path.parent()
                    {
                        open_folder(session, dir);
                    }
                    if ui.button("Delete…").clicked() {
                        self.ask_delete(session, &rel);
                    }
                });
                return;
            }
            self.selected_scene = None;
        }
        ui.horizontal(|ui| {
            let Some(entry) = entry else {
                let dir = session.project.resolve(&self.folder);
                ui.label(RichText::new(dir.display().to_string()).weak());
                if ui.button("Show in folder").clicked() {
                    open_folder(session, &dir);
                }
                return;
            };
            let rect = icons::row_rect(ui, 14.0);
            icons::paint(
                ui.painter(),
                rect,
                kind_icon(entry.kind),
                kind_color(entry.kind),
            );
            ui.label(&entry.relative);
            if let Some((color, text)) = status_of(entry, resources) {
                let first = text.lines().next().unwrap_or_default().to_owned();
                ui.colored_label(color, first).on_hover_text(text);
            }
            if entry.kind == AssetKind::Gltf
                && ui
                    .button("Add to scene")
                    .on_hover_text("Or drag the tile into the viewport")
                    .clicked()
            {
                session.add_model(entry.id, camera.spawn_point(4.0));
            }
            if entry.kind == AssetKind::Audio {
                ui.separator();
                audio.controls(ui, session, entry, mixer);
                ui.separator();
            }
            if entry.kind != AssetKind::Audio
                && ui
                    .button("Reimport")
                    .on_hover_text("Read the file again")
                    .clicked()
            {
                self.thumbnails.remove(&entry.id);
                actions.push(Action::Reimport(entry.id));
            }
            if ui
                .button("Open")
                .on_hover_text("Opens the file in the program your system uses for it")
                .clicked()
            {
                open_file(session, &entry.relative);
            }
            if ui.button("Show in folder").clicked()
                && let Some(dir) = entry.source.parent()
            {
                open_folder(session, dir);
            }
            if ui.button("Delete…").clicked() {
                self.ask_delete(session, &entry.relative);
            }
        });
    }

    /// The details strip for a script or other plain file.
    fn file_details(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, rel: &str) {
        let kind = FileKind::of(rel);
        ui.horizontal(|ui| {
            let rect = icons::row_rect(ui, 14.0);
            let (icon, color) = file_icon(kind);
            icons::paint(ui.painter(), rect, icon, color);
            ui.label(rel);
            if let Some(behaviour) = script_kind(rel) {
                ui.label(RichText::new(format!("behaviour {behaviour}")).weak());
            }
            if ui
                .button("Open")
                .on_hover_text("Opens the file in your text editor")
                .clicked()
            {
                open_file(session, rel);
            }
            if ui.button("Show in folder").clicked()
                && let Some(dir) = session.project.resolve(rel).parent()
            {
                open_folder(session, dir);
            }
            if ui.button("Delete…").clicked() {
                self.ask_delete(session, rel);
            }
        });
    }
}

/// Opens a project file in the program the system uses for it.
fn open_file(session: &mut EditorSession, rel: &str) {
    crate::scripting::open_in_editor(session, rel);
}

fn open_folder(session: &mut EditorSession, dir: &Path) {
    if let Err(e) = copperframe_core::report::open_external(&dir.display().to_string()) {
        session.error(format!("could not open the folder: {e}"));
    }
}

/// Shortens `name` to about `width` points.
fn elide(name: &str, width: f32) -> String {
    let max = ((width / 6.0) as usize).max(4);
    if name.chars().count() <= max {
        return name.to_owned();
    }
    let head: String = name.chars().take(max - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subfolders_are_direct_children() {
        let all: Vec<String> = [
            "assets",
            "assets/props",
            "assets/props/big",
            "assets/sounds",
            "scenes",
            "scripts",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            subfolders(&all, "assets"),
            ["assets/props", "assets/sounds"]
        );
        assert!(subfolders(&all, "scripts").is_empty());
    }

    #[test]
    fn scripts_are_listed_by_folder_or_searched_everywhere() {
        let a = "scripts/puzzles/door.lua";
        assert!(file_listed(a, "scripts/puzzles", "", Filter::All));
        assert!(!file_listed(a, "scripts", "", Filter::All));
        assert!(file_listed(a, "assets", "DOOR", Filter::Scripts));
        assert!(file_listed(a, "assets", "", Filter::Scripts));
        assert!(!file_listed(
            "assets/notes.txt",
            "assets",
            "",
            Filter::Scripts
        ));
        assert!(file_listed("assets/notes.txt", "assets", "", Filter::All));
        assert!(!file_listed(a, "scripts/puzzles", "", Filter::Scenes));
        assert!(!file_listed(
            a,
            "scripts/puzzles",
            "",
            Filter::Kind(AssetKind::Audio)
        ));
        assert_eq!(script_kind(a).as_deref(), Some("lua.puzzles.door"));
        assert_eq!(
            script_kind("scripts/Puzzles/DoorOpener.cs").as_deref(),
            Some("cs.door_opener")
        );
        assert_eq!(script_kind("assets/notes.txt"), None);
        assert!(!scene_listed(
            "scenes/a.scene.json",
            "scenes",
            "",
            Filter::Scripts
        ));
    }

    #[test]
    fn uses_read_naturally() {
        assert_eq!(
            describe_uses(&[("scenes/a.scene.json".into(), 2)]),
            "1 scene (scenes/a.scene.json ×2)"
        );
        assert_eq!(
            describe_uses(&[
                ("scenes/a.scene.json".into(), 1),
                ("scenes/b.scene.json".into(), 1)
            ]),
            "2 scenes (scenes/a.scene.json, scenes/b.scene.json)"
        );
    }

    #[test]
    fn grid_lists_the_open_folder_or_searches_below_it() {
        assert!(listed("assets/props/crate.glb", "assets/props", ""));
        assert!(!listed("assets/props/big/crate.glb", "assets/props", ""));
        assert!(listed(
            "assets/props/big/Crate.glb",
            "assets/props",
            "crate"
        ));
        assert!(!listed("assets/sounds/crate.wav", "assets/props", "crate"));
        assert!(!listed("assets/propsy/crate.glb", "assets/props", "crate"));
        assert!(!listed("assets/props/crate.glb", "assets", "pillar"));
    }

    #[test]
    fn scenes_are_listed_by_folder_or_searched_everywhere() {
        let a = "scenes/levels/Cave.scene.json";
        assert_eq!(scene_label(a), "Cave");
        assert!(in_scenes("scenes/levels") && in_scenes("scenes") && !in_scenes("scenesx"));
        assert!(scene_listed(a, "scenes/levels", "", Filter::All));
        assert!(!scene_listed(a, "scenes", "", Filter::All));
        assert!(scene_listed(a, "scenes", "cave", Filter::All));
        assert!(scene_listed(a, "assets", "CAVE", Filter::Scenes));
        assert!(!scene_listed(a, "assets", "lake", Filter::Scenes));
        assert!(!scene_listed(
            a,
            "scenes/levels",
            "",
            Filter::Kind(AssetKind::Gltf)
        ));
    }

    #[test]
    fn long_names_are_shortened() {
        assert_eq!(elide("crate.glb", 100.0), "crate.glb");
        assert_eq!(elide("machine_hum_loop.wav", 48.0), "machine…");
    }
}
