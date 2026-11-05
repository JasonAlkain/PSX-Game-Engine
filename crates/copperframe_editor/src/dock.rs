//! The dockable panels. Every panel is a tab that can be dragged to another
//! group or edge, resized by its borders, and closed with its ×. Closing only
//! hides a panel, so the Window menu brings it back where it was.
//!
//! The layout and toolbar settings are kept in `editor_layout.json` in the
//! engine's data folder and apply to every project.

use std::path::{Path, PathBuf};

use egui_tiles::{Container, Tile, TileId, Tiles, Tree, UiResponse};
use serde::{Deserialize, Serialize};

use crate::theme;

/// `format` tag of the layout file.
pub const LAYOUT_FORMAT: &str = "copperframe_editor_layout";
pub const LAYOUT_VERSION: u32 = 1;
pub const LAYOUT_FILE: &str = "editor_layout.json";

/// An editor panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Viewport,
    Hierarchy,
    Inspector,
    SceneSettings,
    Assets,
    Console,
    ProjectSettings,
    AudioMixer,
    Problems,
}

impl Pane {
    pub const ALL: [Pane; 9] = [
        Pane::Viewport,
        Pane::Hierarchy,
        Pane::Inspector,
        Pane::SceneSettings,
        Pane::Assets,
        Pane::Console,
        Pane::ProjectSettings,
        Pane::AudioMixer,
        Pane::Problems,
    ];

    /// Panels that start closed in the standard layout.
    pub fn starts_closed(self) -> bool {
        matches!(
            self,
            Pane::ProjectSettings | Pane::AudioMixer | Pane::Problems
        )
    }

    /// The panel whose group this one joins when an older layout lacks it.
    fn neighbour(self) -> Pane {
        match self {
            Pane::AudioMixer | Pane::Problems => Pane::Assets,
            _ => Pane::Inspector,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Pane::Viewport => "Viewport",
            Pane::Hierarchy => "Hierarchy",
            Pane::Inspector => "Inspector",
            Pane::SceneSettings => "Scene settings",
            Pane::Assets => "Assets",
            Pane::Console => "Console",
            Pane::ProjectSettings => "Project settings",
            Pane::AudioMixer => "Audio mixer",
            Pane::Problems => "Problems",
        }
    }
}

/// Draws the panels. The dock supplies the frame around each panel's content.
pub trait PaneContents {
    fn pane(&mut self, ui: &mut egui::Ui, pane: Pane);
}

/// The panel layout.
#[derive(Debug, Clone)]
pub struct Dock {
    pub tree: Tree<Pane>,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            tree: default_tree(),
        }
    }
}

const TREE_ID: &str = "copperframe-dock";

/// Hierarchy and viewport side by side over the assets and console, with the
/// inspector and scene settings down the right. Project settings shares the
/// right-hand group, and the Problems panel and the audio mixer the bottom
/// one; they start closed.
fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let tabs = |tiles: &mut Tiles<Pane>, panes: &[Pane]| {
        let children = panes.iter().map(|p| tiles.insert_pane(*p)).collect();
        tiles.insert_tab_tile(children)
    };
    let hierarchy = tabs(&mut tiles, &[Pane::Hierarchy]);
    let viewport = tabs(&mut tiles, &[Pane::Viewport]);
    let bottom = tabs(
        &mut tiles,
        &[
            Pane::Assets,
            Pane::Console,
            Pane::Problems,
            Pane::AudioMixer,
        ],
    );
    let right = tabs(
        &mut tiles,
        &[Pane::Inspector, Pane::SceneSettings, Pane::ProjectSettings],
    );

    let top = tiles.insert_horizontal_tile(vec![hierarchy, viewport]);
    set_share(&mut tiles, top, hierarchy, 1.0);
    set_share(&mut tiles, top, viewport, 4.0);
    let left = tiles.insert_vertical_tile(vec![top, bottom]);
    set_share(&mut tiles, left, top, 5.0);
    set_share(&mut tiles, left, bottom, 2.0);
    let root = tiles.insert_horizontal_tile(vec![left, right]);
    set_share(&mut tiles, root, left, 3.4);
    set_share(&mut tiles, root, right, 1.0);
    let mut tree = Tree::new(egui::Id::unique(TREE_ID), root, tiles);
    for pane in Pane::ALL.into_iter().filter(|p| p.starts_closed()) {
        if let Some(id) = tree.tiles.find_pane(&pane) {
            tree.set_visible(id, false);
        }
    }
    tree
}

/// Adds panels a layout saved by an older editor does not have, closed, to
/// the group holding their neighbour (or beside the root).
fn add_missing_panes(tree: &mut Tree<Pane>) {
    for pane in Pane::ALL {
        if tree.tiles.find_pane(&pane).is_some() {
            continue;
        }
        let id = tree.tiles.insert_pane(pane);
        tree.set_visible(id, false);
        let group = tree
            .tiles
            .find_pane(&pane.neighbour())
            .and_then(|n| tree.tiles.parent_of(n));
        let added = match group.and_then(|g| tree.tiles.get_mut(g)) {
            Some(Tile::Container(c)) => {
                c.add_child(id);
                true
            }
            _ => false,
        };
        if !added {
            match tree.root.and_then(|r| tree.tiles.get_mut(r)) {
                Some(Tile::Container(c)) => c.add_child(id),
                _ => {
                    let tabs = tree.tiles.insert_tab_tile(vec![id]);
                    tree.root = Some(tabs);
                }
            }
        }
    }
}

fn set_share(tiles: &mut Tiles<Pane>, container: TileId, child: TileId, share: f32) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        linear.shares.set_share(child, share);
    }
}

impl Dock {
    /// Moves the layout out, leaving an empty one until it is put back.
    pub fn take(&mut self) -> Dock {
        std::mem::replace(
            self,
            Dock {
                tree: Tree::empty(egui::Id::unique(TREE_ID)),
            },
        )
    }

    pub fn reset(&mut self) {
        self.tree = default_tree();
    }

    pub fn is_open(&self, pane: Pane) -> bool {
        self.tree
            .tiles
            .find_pane(&pane)
            .is_some_and(|id| self.tree.is_visible(id))
    }

    /// Opens a panel where it last was and brings it to the front of its
    /// group, or closes it.
    pub fn set_open(&mut self, pane: Pane, open: bool) {
        let Some(id) = self.tree.tiles.find_pane(&pane) else {
            return;
        };
        self.tree.set_visible(id, open);
        if open {
            self.tree
                .make_active(|_, tile| matches!(tile, Tile::Pane(p) if *p == pane));
        }
    }

    /// Whether any panel is showing.
    pub fn any_open(&self) -> bool {
        Pane::ALL.iter().any(|&p| self.is_open(p))
    }

    /// Draws the panels into `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, contents: &mut dyn PaneContents) {
        let mut behavior = Behavior { contents };
        self.tree.ui(&mut behavior, ui);
    }
}

struct Behavior<'a> {
    contents: &'a mut dyn PaneContents,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        let rect = ui.max_rect();
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        if *pane == Pane::Viewport {
            // The image is underneath egui: no background here.
            self.contents.pane(ui, *pane);
        } else {
            ui.painter().rect_filled(rect, 0.0, theme::PANEL);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.contents.pane(ui, *pane);
                });
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile: TileId) -> bool {
        matches!(tiles.get(tile), Some(Tile::Pane(_)))
    }

    /// Hides the panel; returning false keeps its place in the layout.
    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile: TileId) -> bool {
        tiles.set_visible(tile, false);
        false
    }

    fn close_button_outer_size(&self) -> f32 {
        14.0
    }

    fn close_button_inner_margin(&self) -> f32 {
        3.0
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        26.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }

    fn min_size(&self) -> f32 {
        60.0
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::BAR
    }

    fn tab_bg_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::PANEL
        } else {
            theme::BAR
        }
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Stroke {
        if state.active {
            egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7))
        } else {
            egui::Stroke::NONE
        }
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7))
    }

    fn tab_text_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        }
    }

    fn drag_preview_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::ACCENT.gamma_multiply(0.35)
    }

    fn drag_preview_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.0, theme::ACCENT)
    }
}

/// The contents of the layout file.
#[derive(Serialize, Deserialize)]
struct LayoutFile<P> {
    format: String,
    version: u32,
    tree: Tree<Pane>,
    prefs: P,
}

/// Where the layout is kept for this user.
pub fn layout_path() -> Option<PathBuf> {
    copperframe_core::report::data_dir()
        .or_else(copperframe_core::report::engine_data_dir)
        .map(|d| d.join(LAYOUT_FILE))
}

/// Reads a saved layout. A missing, damaged or newer file gives `None`: the
/// layout is a convenience and never stops the editor from opening.
pub fn load_layout<P: for<'de> Deserialize<'de>>(path: &Path) -> Option<(Dock, P)> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: LayoutFile<P> = serde_json::from_str(&text).ok()?;
    if file.format != LAYOUT_FORMAT || file.version != LAYOUT_VERSION {
        return None;
    }
    let mut dock = Dock { tree: file.tree };
    // A layout without a panel (saved before it existed) could never show
    // it again.
    add_missing_panes(&mut dock.tree);
    Pane::ALL
        .iter()
        .all(|p| dock.tree.tiles.find_pane(p).is_some())
        .then_some((dock, file.prefs))
}

pub fn save_layout<P: Serialize + Clone>(
    dock: &Dock,
    prefs: &P,
    path: &Path,
) -> Result<(), String> {
    let file = LayoutFile {
        format: LAYOUT_FORMAT.to_owned(),
        version: LAYOUT_VERSION,
        tree: dock.tree.clone(),
        prefs: prefs.clone(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())? + "\n";
    copperframe_core::fsutil::atomic_write(path, text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::{Tool, ToolPrefs};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cf_dock_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(LAYOUT_FILE)
    }

    #[test]
    fn default_layout_has_every_panel_once() {
        let mut dock = Dock::default();
        for pane in Pane::ALL {
            assert_eq!(dock.is_open(pane), !pane.starts_closed(), "{pane:?}");
        }
        dock.set_open(Pane::ProjectSettings, true);
        assert!(dock.is_open(Pane::ProjectSettings));
        assert!(dock.is_open(Pane::Viewport));
    }

    #[test]
    fn layouts_from_before_project_settings_get_it_closed() {
        let path = temp("old");
        let mut dock = Dock::default();
        dock.set_open(Pane::Console, false);
        let id = dock.tree.tiles.find_pane(&Pane::ProjectSettings).unwrap();
        dock.tree.remove_recursively(id);
        assert!(dock.tree.tiles.find_pane(&Pane::ProjectSettings).is_none());
        save_layout(&dock, &ToolPrefs::default(), &path).unwrap();
        let (mut loaded, _): (Dock, ToolPrefs) = load_layout(&path).expect("kept");
        assert!(!loaded.is_open(Pane::Console), "the user's layout survives");
        assert!(!loaded.is_open(Pane::ProjectSettings));
        loaded.set_open(Pane::ProjectSettings, true);
        assert!(loaded.is_open(Pane::ProjectSettings));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn closing_hides_and_reopening_restores_the_same_tile() {
        let mut dock = Dock::default();
        let id = dock.tree.tiles.find_pane(&Pane::Console);
        dock.set_open(Pane::Console, false);
        assert!(!dock.is_open(Pane::Console));
        assert!(dock.is_open(Pane::Assets), "the others stay");
        dock.set_open(Pane::Console, true);
        assert!(dock.is_open(Pane::Console));
        assert_eq!(dock.tree.tiles.find_pane(&Pane::Console), id);
        for pane in Pane::ALL {
            dock.set_open(pane, false);
        }
        assert!(!dock.any_open());
        dock.reset();
        assert!(dock.any_open());
    }

    #[test]
    fn layout_and_prefs_round_trip() {
        let path = temp("round_trip");
        let mut dock = Dock::default();
        dock.set_open(Pane::Console, false);
        let prefs = ToolPrefs {
            tool: Tool::Rotate,
            snap_rotate: 45.0,
            ..Default::default()
        };
        save_layout(&dock, &prefs, &path).unwrap();
        let (loaded, loaded_prefs): (Dock, ToolPrefs) = load_layout(&path).unwrap();
        assert!(!loaded.is_open(Pane::Console));
        assert!(loaded.is_open(Pane::Viewport));
        assert_eq!(loaded_prefs, prefs);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn unusable_layout_files_are_ignored() {
        let path = temp("bad");
        assert!(load_layout::<ToolPrefs>(&path).is_none(), "missing");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(load_layout::<ToolPrefs>(&path).is_none(), "damaged");
        save_layout(&Dock::default(), &ToolPrefs::default(), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let newer = text.replace("\"version\": 1", "\"version\": 99");
        std::fs::write(&path, newer).unwrap();
        assert!(load_layout::<ToolPrefs>(&path).is_none(), "newer version");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
