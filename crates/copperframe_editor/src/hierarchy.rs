//! The Hierarchy panel: the scene's entities as a tree.
//!
//! - A search box filters by name (case-insensitive). Matches are shown
//!   with their ancestors, dimmed, so it is clear where they sit.
//! - Parents expand and collapse; the state is kept per entity for the
//!   whole editor session, so it survives switching scenes and back.
//! - Rows drag and drop: onto the upper or lower edge of a row places the
//!   entity before or after it, onto the middle makes it a child, and onto
//!   the empty space below the rows moves it to the end of the scene root.
//!   Every move is one [`Command::MoveEntity`], so it is validated (no
//!   cycles, depth limit, root-only rigidbodies) and undoable, and the
//!   order is what the scene file stores.
//! - Double-click or F2 renames in place; right-click opens a menu.
//!
//! Rows are laid out by hand so only the visible ones cost anything,
//! which keeps scenes with thousands of entities responsive.

use std::collections::{HashMap, HashSet};

use copperframe_core::EntityId;
use copperframe_scene::{Command, PrimitiveShape, Scene, Transform};
use egui::{Color32, Key, Pos2, Rect, Sense, Stroke, vec2};

use crate::session::EditorSession;
use crate::theme;
use crate::viewport::EditorCamera;

const ROW_H: f32 = 20.0;
const INDENT: f32 = 14.0;
/// Space before the first indent (room for the root's arrow).
const GUTTER: f32 = 4.0;

/// What a drag from the hierarchy carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HierarchyDrag(pub EntityId);

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: EntityId,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    /// False for ancestors shown only for context while searching.
    pub matches: bool,
}

/// Where a dragged entity would land, relative to a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPlace {
    Before(EntityId),
    After(EntityId),
    Into(EntityId),
    /// The end of the scene root.
    RootEnd,
}

/// Children of every parent, in list (display) order.
fn child_lists(scene: &Scene) -> HashMap<Option<EntityId>, Vec<EntityId>> {
    let mut map: HashMap<Option<EntityId>, Vec<EntityId>> = HashMap::new();
    for e in &scene.entities {
        map.entry(e.parent).or_default().push(e.id);
    }
    map
}

/// The rows to show. Without a search: the tree, skipping the children of
/// collapsed entities. With one: entities whose name contains it (ignoring
/// case), with every ancestor shown and expanded.
pub fn visible_rows(scene: &Scene, collapsed: &HashSet<EntityId>, search: &str) -> Vec<Row> {
    let kids = child_lists(scene);
    let needle = search.trim().to_lowercase();
    // While searching: matches and their ancestors.
    let shown: Option<(HashSet<EntityId>, HashSet<EntityId>)> = (!needle.is_empty()).then(|| {
        let mut matches = HashSet::new();
        let mut shown = HashSet::new();
        for e in &scene.entities {
            if e.name.to_lowercase().contains(&needle) {
                matches.insert(e.id);
                let mut cur = Some(e.id);
                // Bounded walk: a broken file could hold a cycle.
                for _ in 0..=copperframe_scene::scene::MAX_DEPTH {
                    let Some(c) = cur else { break };
                    if !shown.insert(c) {
                        break;
                    }
                    cur = scene.get(c).and_then(|e| e.parent);
                }
            }
        }
        (matches, shown)
    });
    let mut rows = Vec::new();
    let mut stack: Vec<(EntityId, usize)> = kids
        .get(&None)
        .map(|r| r.iter().rev().map(|&id| (id, 0)).collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some((id, depth)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let children = kids.get(&Some(id)).map(Vec::as_slice).unwrap_or(&[]);
        let (matches, expanded, visible_kids): (bool, bool, Vec<EntityId>) = match &shown {
            None => {
                let expanded = !collapsed.contains(&id);
                (true, expanded, children.to_vec())
            }
            Some((matches, shown)) => {
                if !shown.contains(&id) {
                    continue;
                }
                let kids: Vec<EntityId> = children
                    .iter()
                    .copied()
                    .filter(|k| shown.contains(k))
                    .collect();
                (matches.contains(&id), true, kids)
            }
        };
        rows.push(Row {
            id,
            depth,
            has_children: !children.is_empty(),
            expanded,
            matches,
        });
        if expanded && depth < copperframe_scene::scene::MAX_DEPTH {
            for &k in visible_kids.iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    rows
}

/// The parent and sibling index a drop means, or `None` if the drop is
/// not allowed (onto itself or its own descendants, or a moving body
/// under a parent).
pub fn resolve_drop(
    scene: &Scene,
    dragged: EntityId,
    place: DropPlace,
) -> Option<(Option<EntityId>, usize)> {
    let entity = scene.get(dragged)?;
    let siblings_without = |parent: Option<EntityId>| -> Vec<EntityId> {
        scene
            .children(parent)
            .map(|e| e.id)
            .filter(|&id| id != dragged)
            .collect()
    };
    let (parent, index) = match place {
        DropPlace::RootEnd => (None, usize::MAX),
        DropPlace::Into(target) => (Some(target), usize::MAX),
        DropPlace::Before(target) | DropPlace::After(target) => {
            if target == dragged {
                return None;
            }
            let parent = scene.get(target)?.parent;
            let pos = siblings_without(parent).iter().position(|&s| s == target)?;
            let index = if matches!(place, DropPlace::After(_)) {
                pos + 1
            } else {
                pos
            };
            (parent, index)
        }
    };
    if let Some(p) = parent
        && scene.is_ancestor_or_self(dragged, p)
    {
        return None;
    }
    if parent.is_some() && entity.rigidbody.is_some() {
        return None;
    }
    // Dropping where it already is would be an empty undo step.
    let current = siblings_without(entity.parent);
    let now = scene
        .children(entity.parent)
        .position(|e| e.id == dragged)
        .unwrap_or(0);
    if parent == entity.parent && index.min(current.len()) == now {
        return None;
    }
    Some((parent, index))
}

/// The panel's state, kept in `UiState`.
#[derive(Debug, Default)]
pub struct HierarchyPanel {
    pub search: String,
    collapsed: HashSet<EntityId>,
    /// Entity being renamed in place, and the text typed so far.
    renaming: Option<(EntityId, String)>,
    /// Selection when the panel last drew, to notice selections made
    /// elsewhere (the viewport) and reveal them.
    last_selection: Option<EntityId>,
}

/// Row background when nothing else applies: odd rows a little darker.
fn stripe(index: usize) -> Color32 {
    if index % 2 == 1 {
        theme::ROW_ODD
    } else {
        theme::PANEL
    }
}

impl HierarchyPanel {
    /// Expands every ancestor of `id` so its row is visible.
    pub fn reveal(&mut self, scene: &Scene, id: EntityId) {
        let mut cur = scene.get(id).and_then(|e| e.parent);
        for _ in 0..=copperframe_scene::scene::MAX_DEPTH {
            let Some(c) = cur else { break };
            self.collapsed.remove(&c);
            cur = scene.get(c).and_then(|e| e.parent);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        keep_world: bool,
    ) {
        self.header(ui, session);
        ui.add_space(2.0);

        let rows = visible_rows(&session.scene, &self.collapsed, &self.search);
        // A selection made elsewhere: open its parents and scroll to it.
        let mut scroll_to = None;
        if session.selection != self.last_selection {
            if let Some(id) = session.selection {
                self.reveal(&session.scene, id);
                scroll_to = Some(id);
            }
            self.last_selection = session.selection;
        }
        let rows = if scroll_to.is_some() {
            visible_rows(&session.scene, &self.collapsed, &self.search)
        } else {
            rows
        };

        let hovered_panel = ui.ui_contains_pointer();
        if hovered_panel && self.renaming.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            self.keyboard(ui, session, &rows);
        }

        let mut drop: Option<(EntityId, DropPlace)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let width = ui.available_width();
                let total = rows.len() as f32 * ROW_H;
                // Spare room below the rows: the "move to root" drop area.
                ui.set_height((total + ROW_H).max(viewport.height()));
                let top = ui.min_rect().top();
                let left = ui.min_rect().left();
                let first = ((viewport.min.y / ROW_H).floor().max(0.0)) as usize;
                let last = (((viewport.max.y / ROW_H).ceil()) as usize + 1).min(rows.len());
                let dragging = egui::DragAndDrop::payload::<HierarchyDrag>(ui.ctx()).map(|d| d.0);

                if let Some(id) = scroll_to
                    && let Some(i) = rows.iter().position(|r| r.id == id)
                {
                    let rect = Rect::from_min_size(
                        Pos2::new(left, top + i as f32 * ROW_H),
                        vec2(width, ROW_H),
                    );
                    ui.scroll_to_rect(rect, None);
                }

                for (i, row) in rows.iter().enumerate().take(last).skip(first) {
                    let rect = Rect::from_min_size(
                        Pos2::new(left, top + i as f32 * ROW_H),
                        vec2(width, ROW_H),
                    );
                    if let Some(d) = self.row(ui, session, camera, rect, i, row, dragging) {
                        drop = Some(d);
                    }
                }

                // Empty space under the rows.
                let rest = Rect::from_min_max(
                    Pos2::new(left, top + total),
                    Pos2::new(
                        left + width,
                        ui.min_rect().bottom().max(top + total + ROW_H),
                    ),
                );
                let response = ui.interact(rest, ui.scope_id().with("rest"), Sense::click());
                if response.clicked() {
                    session.selection = None;
                }
                response.context_menu(|ui| create_menu(ui, session, camera, None));
                if let Some(d) = dragging {
                    if response.dnd_hover_payload::<HierarchyDrag>().is_some() {
                        let ok = resolve_drop(&session.scene, d, DropPlace::RootEnd).is_some();
                        let y = top + total + 1.0;
                        ui.painter().hline(
                            rest.x_range(),
                            y,
                            Stroke::new(2.0, if ok { theme::ACCENT } else { theme::INVALID }),
                        );
                    }
                    if response.dnd_release_payload::<HierarchyDrag>().is_some() {
                        drop = Some((d, DropPlace::RootEnd));
                    }
                    // Scroll while dragging near the top or bottom edge.
                    if let Some(p) = ui.ctx().pointer_hover_pos() {
                        let clip = ui.clip_rect();
                        if clip.contains(p) {
                            if p.y < clip.top() + ROW_H {
                                ui.scroll_with_delta(vec2(0.0, 6.0));
                            } else if p.y > clip.bottom() - ROW_H {
                                ui.scroll_with_delta(vec2(0.0, -6.0));
                            }
                        }
                    }
                }
                if let Some(d) = dragging {
                    drag_label(ui, session, d);
                }
            });

        if let Some((dragged, place)) = drop
            && let Some((parent, index)) = resolve_drop(&session.scene, dragged, place)
        {
            if let Some(p) = parent {
                self.collapsed.remove(&p);
            }
            session.execute(Command::MoveEntity {
                id: dragged,
                parent,
                index,
                keep_world_transform: keep_world,
            });
            session.selection = Some(dragged);
            self.last_selection = Some(dragged);
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, session: &mut EditorSession) {
        ui.horizontal(|ui| {
            let clear_w = if self.search.is_empty() { 0.0 } else { 22.0 };
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search by name…")
                    .desired_width((ui.available_width() - clear_w - 4.0).max(40.0)),
            );
            if edit.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.search.clear();
            }
            if !self.search.is_empty()
                && ui
                    .small_button("×")
                    .on_hover_text("Clear the search (Esc)")
                    .clicked()
            {
                self.search.clear();
            }
        });
        let needle = self.search.trim().to_lowercase();
        if !needle.is_empty() {
            let n = session
                .scene
                .entities
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&needle))
                .count();
            ui.label(
                egui::RichText::new(format!("{n} of {} match", session.scene.entities.len()))
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
    }

    /// Arrow keys move the selection and open or close parents; F2 renames.
    fn keyboard(&mut self, ui: &egui::Ui, session: &mut EditorSession, rows: &[Row]) {
        let pressed = |k| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        let at = session
            .selection
            .and_then(|s| rows.iter().position(|r| r.id == s));
        if pressed(Key::ArrowDown) {
            let next = at.map_or(0, |i| (i + 1).min(rows.len().saturating_sub(1)));
            session.selection = rows.get(next).map(|r| r.id);
        } else if pressed(Key::ArrowUp) {
            let next = at.map_or(0, |i| i.saturating_sub(1));
            session.selection = rows.get(next).map(|r| r.id);
        } else if let Some(i) = at {
            let row = &rows[i];
            if pressed(Key::ArrowRight) && row.has_children {
                self.collapsed.remove(&row.id);
            } else if pressed(Key::ArrowLeft) {
                if row.has_children && row.expanded {
                    self.collapsed.insert(row.id);
                } else if let Some(p) = session.scene.get(row.id).and_then(|e| e.parent) {
                    session.selection = Some(p);
                }
            } else if pressed(Key::F2) {
                self.start_rename(session, row.id);
            }
        }
        // Keep the panel's own moves from scrolling as if made elsewhere.
        if session.selection != self.last_selection {
            self.last_selection = None;
        }
    }

    fn start_rename(&mut self, session: &EditorSession, id: EntityId) {
        if let Some(e) = session.scene.get(id) {
            self.renaming = Some((id, e.name.clone()));
        }
    }

    /// Draws one row and handles its clicks, menu and drops. Returns a drop
    /// released on it.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        camera: &EditorCamera,
        rect: Rect,
        index: usize,
        row: &Row,
        dragging: Option<EntityId>,
    ) -> Option<(EntityId, DropPlace)> {
        let id = row.id;
        let entity = session.scene.get(id)?;
        let name = if entity.name.is_empty() {
            "(unnamed)".to_owned()
        } else {
            entity.name.clone()
        };
        let group = entity.mesh.is_none();
        let response = ui.interact(
            rect,
            ui.scope_id().with(("row", id)),
            Sense::click_and_drag(),
        );
        let selected = session.selection == Some(id);
        let fill = if selected {
            theme::ROW_SELECTED
        } else if response.hovered() && dragging.is_none() {
            theme::ROW_HOVER
        } else {
            stripe(index)
        };
        let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 0.0, fill);
        if selected {
            painter.rect_filled(
                Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
                0.0,
                theme::ACCENT,
            );
        }
        let indent = rect.left() + GUTTER + row.depth as f32 * INDENT;
        let arrow =
            Rect::from_center_size(Pos2::new(indent + 6.0, rect.center().y), vec2(12.0, 12.0));
        if row.has_children {
            paint_arrow(&painter, arrow, row.expanded, theme::TEXT_DIM);
        }
        let text_color = if row.matches {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        let label_left = indent + 16.0;

        // Rename in place.
        let renaming = matches!(&self.renaming, Some((r, _)) if *r == id);
        if renaming {
            let edit_rect = Rect::from_min_max(
                Pos2::new(label_left, rect.top() + 1.0),
                rect.max - vec2(2.0, 1.0),
            );
            let mut finished = None;
            if let Some((_, text)) = &mut self.renaming {
                let edit = ui.put(
                    edit_rect,
                    egui::TextEdit::singleline(text).margin(vec2(2.0, 0.0)),
                );
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    finished = Some(None);
                } else if edit.lost_focus() {
                    finished = Some(Some(text.clone()));
                }
            }
            if let Some(result) = finished {
                self.renaming = None;
                if let Some(new) = result
                    && new != entity.name
                {
                    session.execute(Command::Rename { id, name: new });
                }
            }
        } else {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let mut job = egui::text::LayoutJob::single_section(
                name.clone(),
                egui::TextFormat {
                    font_id: font,
                    color: text_color,
                    italics: group,
                    ..Default::default()
                },
            );
            job.wrap.max_width = (rect.right() - label_left - 4.0).max(10.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = painter.layout_job(job);
            let pos = Pos2::new(label_left, rect.center().y - galley.size().y * 0.5);
            painter.galley(pos, galley, text_color);
        }

        // Clicks.
        if response.clicked() {
            let on_arrow = response
                .interact_pointer_pos()
                .is_some_and(|p| arrow.expand(3.0).contains(p));
            if on_arrow && row.has_children {
                self.toggle(id);
            } else {
                session.selection = Some(id);
                self.last_selection = Some(id);
            }
        }
        if response.double_clicked() && !renaming {
            let on_arrow = response
                .interact_pointer_pos()
                .is_some_and(|p| arrow.expand(3.0).contains(p));
            if !on_arrow {
                self.start_rename(session, id);
            }
        }
        if !renaming {
            response.dnd_set_drag_payload(HierarchyDrag(id));
        }
        response.context_menu(|ui| {
            session.selection = Some(id);
            self.last_selection = Some(id);
            if ui.button("Rename   F2").clicked() {
                self.start_rename(session, id);
                ui.close();
            }
            if ui.button("Duplicate   Ctrl+D").clicked() {
                session.duplicate_selected();
                self.last_selection = session.selection;
                ui.close();
            }
            if ui.button("Delete   Del").clicked() {
                session.delete_selected();
                ui.close();
            }
            ui.separator();
            ui.menu_button("Create child", |ui| {
                create_menu(ui, session, camera, Some(id));
                self.collapsed.remove(&id);
            });
            let parented = session.scene.get(id).is_some_and(|e| e.parent.is_some());
            if ui
                .add_enabled(parented, egui::Button::new("Move to scene root"))
                .clicked()
            {
                session.execute(Command::MoveEntity {
                    id,
                    parent: None,
                    index: usize::MAX,
                    keep_world_transform: true,
                });
                ui.close();
            }
            ui.separator();
            if ui.button("Expand all").clicked() {
                self.collapsed.clear();
                ui.close();
            }
            if ui.button("Collapse all").clicked() {
                self.collapsed = session
                    .scene
                    .entities
                    .iter()
                    .filter_map(|e| e.parent)
                    .collect();
                ui.close();
            }
        });

        // Drops.
        let dragged = dragging?;
        response.dnd_hover_payload::<HierarchyDrag>()?;
        let pointer = ui.ctx().pointer_hover_pos()?;
        let t = (pointer.y - rect.top()) / rect.height();
        let place = if t < 0.25 {
            DropPlace::Before(id)
        } else if t > 0.75 && !(row.has_children && row.expanded) {
            DropPlace::After(id)
        } else {
            DropPlace::Into(id)
        };
        let ok = resolve_drop(&session.scene, dragged, place).is_some();
        let color = if ok { theme::ACCENT } else { theme::INVALID };
        let painter = ui.painter_at(ui.clip_rect());
        let x = (indent + 14.0)..=rect.right();
        match place {
            DropPlace::Before(_) => painter.hline(x, rect.top() + 1.0, Stroke::new(2.0, color)),
            DropPlace::After(_) => painter.hline(x, rect.bottom() - 1.0, Stroke::new(2.0, color)),
            _ => painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                Stroke::new(1.5, color),
                egui::StrokeKind::Inside,
            ),
        };
        if !ok {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NoDrop);
        }
        response
            .dnd_release_payload::<HierarchyDrag>()
            .map(|_| (dragged, place))
    }

    fn toggle(&mut self, id: EntityId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
    }
}

/// The name of the dragged entity beside the pointer.
fn drag_label(ui: &egui::Ui, session: &EditorSession, id: EntityId) {
    let (Some(p), Some(e)) = (ui.ctx().pointer_hover_pos(), session.scene.get(id)) else {
        return;
    };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("hierarchy-drag"),
    ));
    let galley = painter.layout_no_wrap(
        e.name.clone(),
        egui::TextStyle::Body.resolve(ui.style()),
        theme::TEXT,
    );
    let rect = Rect::from_min_size(p + vec2(14.0, 4.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(rect, 3.0, theme::WELL.gamma_multiply(0.9));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(5.0, 2.0), galley, theme::TEXT);
}

/// A right-pointing (closed) or down-pointing (open) triangle.
fn paint_arrow(painter: &egui::Painter, rect: Rect, open: bool, color: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.3;
    let points = if open {
        vec![
            c + vec2(-s, -s * 0.5),
            c + vec2(s, -s * 0.5),
            c + vec2(0.0, s * 0.7),
        ]
    } else {
        vec![
            c + vec2(-s * 0.5, -s),
            c + vec2(-s * 0.5, s),
            c + vec2(s * 0.7, 0.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

/// Create Empty and the primitives, under `parent` (at its origin) or at
/// the scene root in front of the camera.
pub fn create_menu(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    camera: &EditorCamera,
    parent: Option<EntityId>,
) {
    let mut make = |name: &str, shape: Option<PrimitiveShape>| {
        let transform = match parent {
            Some(_) => Transform::IDENTITY,
            None => Transform::from_translation(camera.spawn_point(4.0)),
        };
        let mesh = shape.map(|shape| copperframe_scene::MeshComponent {
            source: copperframe_scene::MeshSource::Primitive {
                shape,
                texture: None,
                tile_size: Some(copperframe_scene::DEFAULT_TILE_SIZE),
            },
            material: Default::default(),
        });
        let created = session.execute(Command::CreateEntity {
            id: None,
            name: name.to_owned(),
            parent,
            transform: Some(transform),
            mesh,
        });
        if let Some(id) = created.and_then(|c| c.first().copied()) {
            session.selection = Some(id);
        }
    };
    if ui.button("Empty").clicked() {
        make("Empty", None);
        ui.close();
    }
    for shape in PrimitiveShape::ALL {
        if ui.button(shape.label()).clicked() {
            make(shape.label(), Some(shape));
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copperframe_scene::{Entity, RigidbodyComponent, apply};

    fn scene() -> (Scene, [EntityId; 5]) {
        // a { a1 { deep } a2 }  b
        let mut s = Scene::new("t");
        let mut add = |name: &str, parent| {
            let mut e = Entity::new(name);
            e.parent = parent;
            let id = e.id;
            s.entities.push(e);
            id
        };
        let a = add("Room", None);
        let a1 = add("Table", Some(a));
        let deep = add("Lamp", Some(a1));
        let a2 = add("Chair", Some(a));
        let b = add("Door", None);
        (s, [a, a1, deep, a2, b])
    }

    fn ids(rows: &[Row]) -> Vec<EntityId> {
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn rows_follow_the_tree_and_collapsing_hides_children() {
        let (s, [a, a1, deep, a2, b]) = scene();
        let rows = visible_rows(&s, &HashSet::new(), "");
        assert_eq!(ids(&rows), [a, a1, deep, a2, b]);
        assert_eq!(
            rows.iter().map(|r| r.depth).collect::<Vec<_>>(),
            [0, 1, 2, 1, 0]
        );
        assert!(rows[0].has_children && !rows[4].has_children);
        let rows = visible_rows(&s, &HashSet::from([a1]), "");
        assert_eq!(ids(&rows), [a, a1, a2, b]);
        assert!(!rows[1].expanded);
        let rows = visible_rows(&s, &HashSet::from([a]), "");
        assert_eq!(ids(&rows), [a, b]);
    }

    #[test]
    fn search_ignores_case_and_keeps_ancestors_for_context() {
        let (s, [a, a1, deep, _, _]) = scene();
        // Collapsed parents still open to show a match.
        let rows = visible_rows(&s, &HashSet::from([a, a1]), "  lAMp ");
        assert_eq!(ids(&rows), [a, a1, deep]);
        assert_eq!(
            rows.iter().map(|r| r.matches).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(visible_rows(&s, &HashSet::new(), "nothing").is_empty());
        // Several matches under one parent.
        let rows = visible_rows(&s, &HashSet::new(), "a");
        assert!(rows.iter().filter(|r| r.matches).count() >= 3);
    }

    #[test]
    fn drops_resolve_to_moves_and_invalid_ones_are_refused() {
        let (mut s, [a, a1, deep, a2, b]) = scene();
        // Before / after a sibling, into a parent, back to the root.
        assert_eq!(resolve_drop(&s, b, DropPlace::Before(a)), Some((None, 0)));
        assert_eq!(
            resolve_drop(&s, a2, DropPlace::Before(a1)),
            Some((Some(a), 0))
        );
        assert_eq!(
            resolve_drop(&s, b, DropPlace::After(a2)),
            Some((Some(a), 2))
        );
        assert_eq!(
            resolve_drop(&s, b, DropPlace::Into(deep)),
            Some((Some(deep), usize::MAX))
        );
        assert_eq!(
            resolve_drop(&s, deep, DropPlace::RootEnd),
            Some((None, usize::MAX))
        );
        // No cycles, no drops onto itself, no no-op moves.
        assert_eq!(resolve_drop(&s, a, DropPlace::Into(deep)), None);
        assert_eq!(resolve_drop(&s, a, DropPlace::Into(a)), None);
        assert_eq!(resolve_drop(&s, a, DropPlace::Before(a)), None);
        assert_eq!(resolve_drop(&s, b, DropPlace::RootEnd), None);
        assert_eq!(resolve_drop(&s, a1, DropPlace::Before(a2)), None);
        // Moving bodies stay at the root.
        s.get_mut(b).unwrap().rigidbody = Some(RigidbodyComponent::default());
        assert_eq!(resolve_drop(&s, b, DropPlace::Into(a)), None);

        // Every resolved drop applies as a valid command.
        s.get_mut(b).unwrap().rigidbody = None;
        let (parent, index) = resolve_drop(&s, b, DropPlace::Before(a1)).unwrap();
        apply(
            &mut s,
            &Command::MoveEntity {
                id: b,
                parent,
                index,
                keep_world_transform: true,
            },
        )
        .unwrap();
        let rows = visible_rows(&s, &HashSet::new(), "");
        assert_eq!(ids(&rows), [a, b, a1, deep, a2]);
    }

    #[test]
    fn deep_hierarchies_list_every_level() {
        let mut s = Scene::new("deep");
        let mut parent = None;
        for i in 0..40 {
            let mut e = Entity::new(format!("n{i}"));
            e.parent = parent;
            parent = Some(e.id);
            s.entities.push(e);
        }
        let rows = visible_rows(&s, &HashSet::new(), "");
        assert_eq!(rows.len(), 40);
        assert_eq!(rows.last().unwrap().depth, 39);
        assert_eq!(visible_rows(&s, &HashSet::new(), "n39").len(), 40);
    }
}
