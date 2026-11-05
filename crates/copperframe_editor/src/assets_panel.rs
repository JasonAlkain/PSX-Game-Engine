//! The asset browser: folders on the left, a grid of tiles on the right, and
//! a strip of details and actions for the selected asset. Models can be
//! dragged from here into the viewport.
//!
//! Scene files are listed too, under their own `scenes` root: double-click
//! opens one, right-click renames, deletes or makes it the start scene, and
//! dragging a scene onto a folder moves it. File operations go through
//! [`EditorSession`] so references to the scene follow it.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use copperframe_assets::project::AudioSettings;
use copperframe_assets::{AssetEntry, AssetKind};
use copperframe_core::AssetId;
use copperframe_runtime::scene_gpu::{AssetStatus, SceneResources};
use egui::{Color32, RichText, Sense, TextureHandle, vec2};

use crate::audio_tools::EditorAudio;
use crate::icons::{self, Icon};
use crate::session::{EditorSession, LogLevel, SCENE_SUFFIX, scene_stem};
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

/// Which kinds of asset the grid lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Kind(AssetKind),
    /// Every scene in the project (a search looks through all of them).
    Scenes,
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
}

/// A dialog the panel shows over the editor.
#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    Rename {
        scene: String,
        name: String,
        error: Option<String>,
    },
    Delete {
        scene: String,
        error: Option<String>,
    },
    NewFolder {
        parent: String,
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

/// Every folder holding assets, with its parents, sorted so that a folder
/// comes right before its children.
fn folders<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut all = BTreeSet::from([ROOT.to_owned(), SCENES.to_owned()]);
    for path in paths {
        let mut dir = folder_of(path);
        while !dir.is_empty() {
            all.insert(dir.to_owned());
            dir = folder_of(dir);
        }
    }
    all.into_iter().collect()
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
        Filter::Kind(_) => false,
        Filter::Scenes => {
            search.is_empty()
                || scene_label(relative)
                    .to_lowercase()
                    .contains(&search.to_lowercase())
        }
        Filter::All => listed(relative, folder, search),
    }
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
        let scene_folders = session.scene_folders();
        let all = folders(
            entries
                .iter()
                .map(|e| e.relative.as_str())
                .chain(scenes.iter().map(String::as_str))
                .chain(scene_folders.iter().map(|f| f.as_str())),
        );
        // `folders` takes file paths; a folder path must count as a folder.
        let mut all = all;
        for f in &scene_folders {
            if !all.contains(f) {
                all.push(f.clone());
            }
        }
        all.sort();
        if !all.contains(&self.folder) {
            self.folder = ROOT.to_owned();
        }
        self.top_bar(ui, session, actions);
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
                    .show(ui, |ui| self.folder_tree(ui, session, &all));
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
        self.dialogs(ui.ctx(), session);
    }

    fn top_bar(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        actions: &mut Vec<Action>,
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
            ] {
                ui.selectable_value(&mut self.filter, filter, label);
            }
            ui.menu_button("Create", |ui| self.create_menu(ui));
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
                        "The assets folder is watched: new and changed files are imported \
                         automatically within a second or two",
                    )
                    .clicked()
                {
                    session.rescan_assets();
                    self.clear_thumbnails();
                    actions.push(Action::AssetsChanged);
                }
            });
        });
    }

    fn folder_tree(&mut self, ui: &mut egui::Ui, session: &mut EditorSession, all: &[String]) {
        for folder in all {
            let depth = folder.matches('/').count();
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                let rect = icons::row_rect(ui, 13.0);
                let icon = if folder == SCENES {
                    Icon::Scene
                } else {
                    Icon::Folder
                };
                icons::paint(ui.painter(), rect, icon, theme::TEXT_DIM);
                let open = self.folder == *folder;
                let response = ui.selectable_label(open, file_name(folder));
                if response.clicked() {
                    self.folder = folder.clone();
                }
                if in_scenes(folder) {
                    self.scene_drop_target(ui, session, &response, folder);
                    response.context_menu(|ui| {
                        if ui.button("Create Scene…").clicked() {
                            self.requests.push(PanelRequest::NewScene(folder.clone()));
                            ui.close();
                        }
                        if ui.button("Create Folder…").clicked() {
                            self.dialog = Some(Dialog::NewFolder {
                                parent: folder.clone(),
                                name: "New folder".into(),
                                error: None,
                            });
                            ui.close();
                        }
                    });
                }
            });
        }
    }

    /// The folder new scenes and scene folders go in: the open folder if
    /// it is under `scenes`, otherwise `scenes`.
    fn scene_folder(&self) -> String {
        if in_scenes(&self.folder) {
            self.folder.clone()
        } else {
            SCENES.to_owned()
        }
    }

    fn create_menu(&mut self, ui: &mut egui::Ui) {
        let folder = self.scene_folder();
        if ui
            .button("Scene…")
            .on_hover_text(format!("A new scene in {folder}/"))
            .clicked()
        {
            self.requests.push(PanelRequest::NewScene(folder.clone()));
            ui.close();
        }
        if ui
            .button("Folder…")
            .on_hover_text(format!("A folder for scenes in {folder}/"))
            .clicked()
        {
            self.dialog = Some(Dialog::NewFolder {
                parent: folder,
                name: "New folder".into(),
                error: None,
            });
            ui.close();
        }
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
        let response = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if search.is_empty() && self.filter != Filter::Scenes {
                for sub in subfolders(all, &self.folder) {
                    any = true;
                    let (response, icon_rect) = self.tile(ui, file_name(sub));
                    let icon = if sub == SCENES {
                        Icon::Scene
                    } else {
                        Icon::Folder
                    };
                    icons::paint(ui.painter(), icon_rect, Icon::Folder, theme::TEXT_DIM);
                    if icon == Icon::Scene {
                        icons::paint(
                            ui.painter(),
                            icon_rect
                                .shrink(icon_rect.width() * 0.3)
                                .translate(vec2(0.0, 3.0)),
                            Icon::Scene,
                            theme::TEXT,
                        );
                    }
                    if response.double_clicked() {
                        self.folder = sub.to_owned();
                    }
                    if in_scenes(sub) {
                        self.scene_drop_target(ui, session, &response, sub);
                    }
                }
            }
            for rel in scenes {
                if !scene_listed(rel, &self.folder, &search, self.filter) {
                    continue;
                }
                any = true;
                self.scene_tile(ui, session, rel);
            }
            if self.filter == Filter::Scenes {
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
                if response.clicked() || response.drag_started() {
                    self.selected = Some(entry.id);
                    self.selected_scene = None;
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
                response.on_hover_text(hover);
            }
        });
        if in_scenes(&self.folder) || self.filter == Filter::Scenes {
            let rest =
                ui.allocate_response(ui.available_size().max(vec2(1.0, 40.0)), Sense::click());
            let menu_target = response.response.union(rest);
            menu_target.context_menu(|ui| self.create_menu(ui));
        }
        if !any {
            let text = if in_scenes(&self.folder) || self.filter == Filter::Scenes {
                "No scenes here. Create one with Create > Scene… or Ctrl+N."
            } else {
                "Nothing here. Files you add to assets/ appear on their own."
            };
            ui.label(RichText::new(text).weak());
        }
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
            self.selected_scene = Some(rel.to_owned());
            self.selected = None;
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
            self.dialog = Some(Dialog::Delete {
                scene: rel.to_owned(),
                error: None,
            });
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
        self.create_menu(ui);
    }

    fn dialogs(&mut self, ctx: &egui::Context, session: &mut EditorSession) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let mut selected = None;
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
                Dialog::Delete { scene, error } => {
                    ui.heading("Delete scene");
                    ui.label(format!(
                        "Delete {scene}? It is moved to .psx/deleted/ in the project folder, \
                         so it can be recovered by hand."
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            match session.delete_scene(&session.project.resolve(scene)) {
                                Ok(()) => close = true,
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
                        match session.create_scene_folder(parent, name) {
                            Ok(p) => {
                                self.folder = session.project.relative(&p);
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
            self.selected_scene = Some(s);
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
                });
                return;
            }
            self.selected_scene = None;
        }
        ui.horizontal(|ui| {
            let Some(entry) = entry else {
                ui.label(RichText::new(session.project.assets_dir().display().to_string()).weak());
                if ui.button("Show in folder").clicked() {
                    let dir = session.project.assets_dir();
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
            if ui.button("Show in folder").clicked()
                && let Some(dir) = entry.source.parent()
            {
                open_folder(session, dir);
            }
        });
    }
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
    fn folders_include_parents_and_sort_before_children() {
        let all = folders(
            [
                "assets/props/big/a.glb",
                "assets/sounds/b.wav",
                "assets/c.png",
            ]
            .into_iter(),
        );
        assert_eq!(
            all,
            [
                "assets",
                "assets/props",
                "assets/props/big",
                "assets/sounds",
                "scenes"
            ]
        );
        assert_eq!(
            subfolders(&all, "assets"),
            ["assets/props", "assets/sounds"]
        );
        assert_eq!(folders(std::iter::empty()), ["assets", "scenes"]);
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
