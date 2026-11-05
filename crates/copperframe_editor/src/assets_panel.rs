//! The asset browser: folders on the left, a grid of tiles on the right, and
//! a strip of details and actions for the selected asset. Models can be
//! dragged from here into the viewport.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use copperframe_assets::{AssetEntry, AssetKind};
use copperframe_core::AssetId;
use copperframe_runtime::scene_gpu::{AssetStatus, SceneResources};
use egui::{Color32, RichText, Sense, TextureHandle, vec2};

use crate::icons::{self, Icon};
use crate::session::{EditorSession, LogLevel};
use crate::theme;
use crate::ui::{Action, level_color};
use crate::viewport::EditorCamera;

const ROOT: &str = copperframe_assets::project::ASSETS_DIR;
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
}

/// The panel's state. Thumbnails are per asset, so they are dropped when
/// assets change or another scene opens.
pub struct AssetsPanel {
    folder: String,
    search: String,
    filter: Filter,
    tile: f32,
    selected: Option<AssetId>,
    thumbnails: HashMap<AssetId, Option<TextureHandle>>,
}

impl Default for AssetsPanel {
    fn default() -> Self {
        Self {
            folder: ROOT.to_owned(),
            search: String::new(),
            filter: Filter::All,
            tile: 72.0,
            selected: None,
            thumbnails: HashMap::new(),
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
    let mut all = BTreeSet::from([ROOT.to_owned()]);
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

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        actions: &mut Vec<Action>,
    ) {
        let entries = session.db.entries.clone();
        let all = folders(entries.iter().map(|e| e.relative.as_str()));
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
                self.details(ui, session, camera, resources, &entries, actions);
            });
        egui::Panel::left("asset-folders")
            .resizable(true)
            .default_size(150.0)
            .min_size(70.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.folder_tree(ui, &all));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.breadcrumb(ui);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.grid(ui, session, camera, resources, &entries, &all);
                });
        });
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
            ] {
                ui.selectable_value(&mut self.filter, filter, label);
            }
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

    fn folder_tree(&mut self, ui: &mut egui::Ui, all: &[String]) {
        for folder in all {
            let depth = folder.matches('/').count();
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                let rect = icons::row_rect(ui, 13.0);
                icons::paint(ui.painter(), rect, Icon::Folder, theme::TEXT_DIM);
                let open = self.folder == *folder;
                if ui.selectable_label(open, file_name(folder)).clicked() {
                    self.folder = folder.clone();
                }
            });
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

    fn grid(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        entries: &[AssetEntry],
        all: &[String],
    ) {
        let mut any = false;
        let mut loaded_one = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if self.search.is_empty() {
                for sub in subfolders(all, &self.folder) {
                    any = true;
                    let (response, icon_rect) = self.tile(ui, file_name(sub));
                    icons::paint(ui.painter(), icon_rect, Icon::Folder, theme::TEXT_DIM);
                    if response.double_clicked() {
                        self.folder = sub.to_owned();
                    }
                }
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
                }
                if entry.kind == AssetKind::Gltf {
                    response.dnd_set_drag_payload(entry.id);
                    if response.double_clicked() {
                        session.add_model(entry.id, camera.spawn_point(4.0));
                    }
                }
                response.on_hover_text(format!("{}\n{}", entry.relative, entry.kind.label()));
            }
        });
        if !any {
            ui.label(
                RichText::new("Nothing here. Files you add to assets/ appear on their own.").weak(),
            );
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

    fn details(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        resources: &SceneResources,
        entries: &[AssetEntry],
        actions: &mut Vec<Action>,
    ) {
        let entry = self
            .selected
            .and_then(|id| entries.iter().find(|e| e.id == id));
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
                "assets/sounds"
            ]
        );
        assert_eq!(
            subfolders(&all, "assets"),
            ["assets/props", "assets/sounds"]
        );
        assert_eq!(folders(std::iter::empty()), ["assets"]);
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
    fn long_names_are_shortened() {
        assert_eq!(elide("crate.glb", 100.0), "crate.glb");
        assert_eq!(elide("machine_hum_loop.wav", 48.0), "machine…");
    }
}
