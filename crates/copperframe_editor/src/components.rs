//! The inspector's component cards and the Add component menu.
//!
//! Every component of the selected entity (built-in ones and each
//! behaviour) is a card with a header that owns reordering (drag the title,
//! or the arrow buttons) and removal. New components come from one Add
//! component button whose menu files them in folders, with a search field.
//! The order is saved in the scene (`component_order`), and behaviours run
//! in it.

use copperframe_assets::AssetKind;
use copperframe_core::EntityId;
use copperframe_runtime::scene_gpu::SceneResources;
use copperframe_scene::{
    AnimatorComponent, AudioSourceComponent, BehaviorComponent, Command, Entity, LightComponent,
    MeshComponent, MeshSource, PrimitiveShape, ShadowComponent, component_key,
};
use egui::{Color32, Id, Key, Stroke, vec2};

use crate::gameplay::{self, GameTools};
use crate::icons::{self, Icon};
use crate::session::EditorSession;
use crate::{theme, ui, visuals};

/// Top-level folders of the Add component menu, which also colour the
/// cards' edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Rendering,
    Lighting,
    Physics,
    Audio,
    Behaviours,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Rendering => "Rendering",
            Group::Lighting => "Lighting",
            Group::Physics => "Physics",
            Group::Audio => "Audio",
            Group::Behaviours => "Behaviours",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Group::Rendering => theme::ACCENT,
            Group::Lighting => Color32::from_rgb(232, 196, 92),
            Group::Physics => Color32::from_rgb(110, 196, 120),
            Group::Audio => Color32::from_rgb(98, 166, 230),
            Group::Behaviours => Color32::from_rgb(178, 132, 222),
        }
    }

    fn of(key: &str) -> Group {
        match key {
            component_key::MESH | component_key::ANIMATOR => Group::Rendering,
            component_key::LIGHT | component_key::SHADOW => Group::Lighting,
            component_key::COLLIDER | component_key::RIGIDBODY => Group::Physics,
            component_key::AUDIO => Group::Audio,
            _ => Group::Behaviours,
        }
    }
}

/// The card title for a component key.
pub fn title(key: &str) -> String {
    match key {
        component_key::MESH => "Mesh".into(),
        component_key::ANIMATOR => "Animator".into(),
        component_key::LIGHT => "Point light".into(),
        component_key::SHADOW => "Blob shadow".into(),
        component_key::COLLIDER => "Collider".into(),
        component_key::RIGIDBODY => "Rigidbody".into(),
        component_key::AUDIO => "Audio source".into(),
        _ => component_key::behavior_kind(key).map_or_else(|| key.to_owned(), behavior_title),
    }
}

/// A readable name for a behaviour kind: `room.caretaker_npc` is
/// "Caretaker npc".
pub fn behavior_title(kind: &str) -> String {
    let last = kind
        .rsplit('.')
        .next()
        .unwrap_or(kind)
        .replace(['_', '-'], " ");
    let mut c = last.trim().chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => kind.to_owned(),
    }
}

/// Moves the item at `from` so it ends up before the item that was at
/// `to` (`to == len` moves it to the end).
pub fn moved(keys: &[String], from: usize, to: usize) -> Vec<String> {
    let mut out = keys.to_vec();
    if from >= out.len() {
        return out;
    }
    let item = out.remove(from);
    let to = if to > from { to - 1 } else { to };
    out.insert(to.min(out.len()), item);
    out
}

/// The command that removes the component named by `key`.
pub fn remove_command(entity: &Entity, key: &str) -> Option<Command> {
    let id = entity.id;
    Some(match key {
        component_key::MESH => Command::SetMesh { id, mesh: None },
        component_key::ANIMATOR => Command::SetAnimator { id, animator: None },
        component_key::LIGHT => Command::SetLight { id, light: None },
        component_key::SHADOW => Command::SetShadow { id, shadow: None },
        component_key::COLLIDER => Command::SetCollider { id, collider: None },
        component_key::RIGIDBODY => Command::SetRigidbody {
            id,
            rigidbody: None,
        },
        component_key::AUDIO => Command::SetAudio { id, audio: None },
        _ => {
            let kind = component_key::behavior_kind(key)?;
            Command::SetBehaviors {
                id,
                behaviors: entity
                    .behaviors
                    .iter()
                    .filter(|b| b.kind != kind)
                    .cloned()
                    .collect(),
            }
        }
    })
}

/// Payload while a card is dragged: the entity and the card's index.
#[derive(Debug, Clone, Copy)]
struct DraggedCard {
    entity: EntityId,
    index: usize,
}

enum CardAction {
    Move { from: usize, to: usize },
    Remove(String),
}

/// Draws the entity's component cards and the Add component button.
pub fn components(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    tools: &mut GameTools,
    id: EntityId,
) {
    let Some(entity) = session.scene.get(id) else {
        return;
    };
    let keys = entity.component_keys();
    let mut action = None;
    for (i, key) in keys.iter().enumerate() {
        // A card's editor may have changed the entity; draw the latest.
        let Some(entity) = session.scene.get(id).cloned() else {
            return;
        };
        if !entity.has_component(key) {
            continue;
        }
        card(ui, i, keys.len(), key, &entity, &mut action, |ui| {
            body(ui, session, resources, tools, &entity, key);
        });
        ui.add_space(4.0);
    }
    if keys.is_empty() {
        ui.label(egui::RichText::new("No components yet.").weak());
    }
    match action {
        Some(CardAction::Move { from, to }) => {
            let order = moved(&keys, from, to);
            if order != keys {
                session.execute(Command::SetComponentOrder { id, order });
            }
        }
        Some(CardAction::Remove(key)) => {
            if let Some(cmd) = session.scene.get(id).and_then(|e| remove_command(e, &key)) {
                session.execute(cmd);
            }
        }
        None => {}
    }
    ui.add_space(2.0);
    add_component_button(ui, session, resources, tools, id);
}

fn body(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    tools: &mut GameTools,
    entity: &Entity,
    key: &str,
) {
    match key {
        component_key::MESH => {
            ui::mesh_editor(ui, session, resources, entity.id, entity.mesh.as_ref());
        }
        component_key::ANIMATOR => visuals::animator_editor(ui, session, resources, entity),
        component_key::LIGHT => visuals::light_editor(ui, session, entity),
        component_key::SHADOW => visuals::shadow_editor(ui, session, entity),
        component_key::COLLIDER => gameplay::collider_editor(ui, session, entity),
        component_key::RIGIDBODY => gameplay::rigidbody_editor(ui, session, entity),
        component_key::AUDIO => {
            let sounds = gameplay::sounds(session);
            gameplay::audio_editor(ui, session, entity, &sounds);
        }
        _ => {
            if let Some(kind) = component_key::behavior_kind(key) {
                gameplay::behavior_editor(ui, session, tools, entity, kind);
            }
        }
    }
}

/// One component card: a coloured edge, a header (collapse toggle, drag
/// handle and title, move and remove buttons) and the editor body.
fn card(
    ui: &mut egui::Ui,
    index: usize,
    count: usize,
    key: &str,
    entity: &Entity,
    action: &mut Option<CardAction>,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let group = Group::of(key);
    let card_id = Id::unique(("component-card", entity.id, key));
    let frame = egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(4)
        .inner_margin(egui::Margin {
            left: 9,
            right: 5,
            top: 4,
            bottom: 6,
        });
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        card_id.with("open"),
        true,
    );
    let response = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
                let drag_id = card_id.with("drag");
                let title_text = title(key);
                let kind = component_key::behavior_kind(key);
                let handle = ui
                    .dnd_drag_source(
                        drag_id,
                        DraggedCard {
                            entity: entity.id,
                            index,
                        },
                        |ui| {
                            // While dragged, the title follows the pointer
                            // as a small tag.
                            let tag = if ui.ctx().is_being_dragged(drag_id) {
                                egui::Frame::new()
                                    .fill(theme::PANEL)
                                    .stroke(Stroke::new(1.0, theme::ACCENT))
                                    .corner_radius(3)
                                    .inner_margin(egui::Margin::symmetric(6, 3))
                            } else {
                                egui::Frame::new()
                            };
                            tag.show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let r = icons::row_rect(ui, 14.0);
                                    icons::paint(ui.painter(), r, Icon::Grip, theme::TEXT_DIM);
                                    ui.label(egui::RichText::new(&title_text).strong());
                                    if let Some(kind) = kind {
                                        ui.label(
                                            egui::RichText::new(kind).small().monospace().weak(),
                                        );
                                    }
                                });
                            });
                        },
                    )
                    .response
                    .on_hover_text("Drag to reorder");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::small_button(ui, Icon::Trash, true, "Remove component").clicked() {
                        *action = Some(CardAction::Remove(key.to_owned()));
                    }
                    if icons::small_button(ui, Icon::ArrowDown, index + 1 < count, "Move down")
                        .clicked()
                    {
                        *action = Some(CardAction::Move {
                            from: index,
                            to: index + 2,
                        });
                    }
                    if icons::small_button(ui, Icon::ArrowUp, index > 0, "Move up").clicked() {
                        *action = Some(CardAction::Move {
                            from: index,
                            to: index - 1,
                        });
                    }
                });
                handle
            });
            header.inner.context_menu(|ui| {
                let mut item = |ui: &mut egui::Ui, enabled: bool, text: &str, a: CardAction| {
                    if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
                        *action = Some(a);
                        ui.close();
                    }
                };
                item(
                    ui,
                    index > 0,
                    "Move to top",
                    CardAction::Move { from: index, to: 0 },
                );
                item(
                    ui,
                    index > 0,
                    "Move up",
                    CardAction::Move {
                        from: index,
                        to: index.saturating_sub(1),
                    },
                );
                item(
                    ui,
                    index + 1 < count,
                    "Move down",
                    CardAction::Move {
                        from: index,
                        to: index + 2,
                    },
                );
                item(
                    ui,
                    index + 1 < count,
                    "Move to bottom",
                    CardAction::Move {
                        from: index,
                        to: count,
                    },
                );
                ui.separator();
                item(ui, true, "Remove", CardAction::Remove(key.to_owned()));
            });
            if state.is_open() {
                ui.add_space(2.0);
                add_body(ui);
            }
        })
        .response;
    state.store(ui.ctx());

    // Coloured edge: which folder of the Add component menu it came from.
    let rect = response.rect;
    let edge = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.left() + 3.0, rect.bottom()),
    );
    ui.painter().rect_filled(
        edge,
        egui::CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        group.color(),
    );

    // Dropping another card of this entity here: above or below the middle.
    if let Some(dragged) = response.dnd_hover_payload::<DraggedCard>()
        && dragged.entity == entity.id
        && let Some(pointer) = ui.ctx().pointer_interact_pos()
    {
        let above = pointer.y < rect.center().y;
        let y = if above {
            rect.top() - 2.0
        } else {
            rect.bottom() + 2.0
        };
        // On top of the cards, so the line shows between them.
        ui.ctx()
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                card_id.with("drop"),
            ))
            .with_clip_rect(ui.clip_rect())
            .hline(rect.x_range(), y, Stroke::new(2.0, theme::ACCENT));
        if let Some(dragged) = response.dnd_release_payload::<DraggedCard>() {
            *action = Some(CardAction::Move {
                from: dragged.index,
                to: if above { index } else { index + 1 },
            });
        }
    }
}

/// What picking an Add component entry does.
#[derive(Clone)]
enum AddAction {
    Run(Command),
    LoadBehaviourKinds,
}

/// One leaf of the Add component menu.
struct Entry {
    /// Folders from the top of the menu, e.g. `["Rendering", "Mesh"]`.
    path: Vec<String>,
    label: String,
    /// Shown on hover.
    help: String,
    /// `Err(reason)` greys the entry out.
    action: Result<AddAction, String>,
}

impl Entry {
    fn new(path: &[&str], label: impl Into<String>, action: Result<AddAction, String>) -> Self {
        Self {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            label: label.into(),
            help: String::new(),
            action,
        }
    }
}

const ALREADY: &str = "the entity already has one";

/// Everything the Add component menu offers for `entity`.
fn entries(
    session: &EditorSession,
    resources: &SceneResources,
    tools: &GameTools,
    entity: &Entity,
) -> Vec<Entry> {
    let id = entity.id;
    let run = |c: Command| Ok(AddAction::Run(c));
    let mut out = Vec::new();

    // Rendering.
    if entity.mesh.is_some() {
        out.push(Entry::new(&["Rendering"], "Mesh", Err(ALREADY.into())));
    } else {
        for shape in PrimitiveShape::ALL {
            out.push(Entry::new(
                &["Rendering", "Mesh"],
                shape.label(),
                run(Command::SetMesh {
                    id,
                    mesh: Some(MeshComponent {
                        source: MeshSource::Primitive {
                            shape,
                            texture: None,
                            tile_size: Some(copperframe_scene::DEFAULT_TILE_SIZE),
                        },
                        material: Default::default(),
                    }),
                }),
            ));
        }
        let models = ui::assets_of(session, AssetKind::Gltf);
        if models.is_empty() {
            out.push(Entry::new(
                &["Rendering", "Mesh"],
                "Model",
                Err("add .gltf or .glb files to assets/ to use models".into()),
            ));
        }
        for (asset, path) in models {
            out.push(Entry::new(
                &["Rendering", "Mesh", "Model"],
                path,
                run(Command::SetMesh {
                    id,
                    mesh: Some(MeshComponent {
                        source: MeshSource::Model { asset },
                        material: Default::default(),
                    }),
                }),
            ));
        }
    }
    let clips = visuals::clips_of(resources, entity).unwrap_or_default();
    if entity.animator.is_some() {
        out.push(Entry::new(&["Rendering"], "Animator", Err(ALREADY.into())));
    } else if clips.is_empty() {
        out.push(Entry::new(
            &["Rendering"],
            "Animator",
            Err("needs a model mesh with a skin and animation clips".into()),
        ));
    } else {
        for c in clips {
            out.push(Entry::new(
                &["Rendering", "Animator"],
                c.clone(),
                run(Command::SetAnimator {
                    id,
                    animator: Some(AnimatorComponent::new(c)),
                }),
            ));
        }
    }

    // Lighting.
    out.push(Entry::new(
        &["Lighting"],
        "Point light",
        if entity.light.is_some() {
            Err(ALREADY.into())
        } else {
            run(Command::SetLight {
                id,
                light: Some(LightComponent::default()),
            })
        },
    ));
    out.push(Entry::new(
        &["Lighting"],
        "Blob shadow",
        if entity.shadow.is_some() {
            Err(ALREADY.into())
        } else {
            run(Command::SetShadow {
                id,
                shadow: Some(ShadowComponent::default()),
            })
        },
    ));

    // Physics.
    out.push(Entry::new(
        &["Physics"],
        "Collider",
        if entity.collider.is_some() {
            Err(ALREADY.into())
        } else {
            run(Command::SetCollider {
                id,
                collider: Some(gameplay::default_collider(entity)),
            })
        },
    ));
    out.push(Entry::new(
        &["Physics"],
        "Rigidbody",
        if entity.rigidbody.is_some() {
            Err(ALREADY.into())
        } else if entity.parent.is_some() {
            Err("rigidbodies go on root entities: unparent it first".into())
        } else if entity
            .collider
            .is_some_and(|c| c.shape == copperframe_scene::ColliderShape::Mesh)
        {
            Err("mesh colliders are static: pick a box, sphere or capsule first".into())
        } else {
            run(Command::SetRigidbody {
                id,
                rigidbody: Some(copperframe_scene::RigidbodyComponent::dynamic()),
            })
        },
    ));

    // Audio.
    let sounds = gameplay::sounds(session);
    if entity.audio.is_some() {
        out.push(Entry::new(&["Audio"], "Audio source", Err(ALREADY.into())));
    } else if sounds.is_empty() {
        out.push(Entry::new(
            &["Audio"],
            "Audio source",
            Err("add .wav or .ogg files to assets/ to use sounds".into()),
        ));
    } else {
        for (asset, path) in sounds {
            out.push(Entry::new(
                &["Audio", "Audio source"],
                path,
                run(Command::SetAudio {
                    id,
                    audio: Some(AudioSourceComponent::new(asset)),
                }),
            ));
        }
    }

    // Behaviours, in the folders the game registered them in.
    match tools.schema() {
        Some(schema) => {
            for (kind, folder, description) in gameplay::behavior_kinds(schema) {
                let mut path = vec!["Behaviours"];
                path.extend(folder.split('/').map(str::trim).filter(|f| !f.is_empty()));
                let action = if entity.behavior(&kind).is_some() {
                    Err(ALREADY.into())
                } else {
                    let mut behaviors = entity.behaviors.clone();
                    behaviors.push(BehaviorComponent::new(kind.clone()));
                    run(Command::SetBehaviors { id, behaviors })
                };
                let mut e = Entry::new(&path, behavior_title(&kind), action);
                e.help = format!("{kind}\n{description}");
                out.push(e);
            }
            out.push(Entry {
                help: "Rebuilds the game and reads its behaviour kinds again".into(),
                ..Entry::new(
                    &["Behaviours"],
                    "Reload behaviour kinds",
                    Ok(AddAction::LoadBehaviourKinds),
                )
            });
        }
        None => {
            let action = if tools.busy_label().is_some() {
                Err("reading the game's behaviour kinds…".into())
            } else {
                Ok(AddAction::LoadBehaviourKinds)
            };
            out.push(Entry {
                help: "Builds the game and reads the behaviour kinds it registers".into(),
                ..Entry::new(&["Behaviours"], "Load behaviour kinds", action)
            });
        }
    }
    out
}

/// Entries whose label, folders or help contain every word of `filter`.
fn matching<'a>(entries: &'a [Entry], filter: &str) -> Vec<&'a Entry> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    entries
        .iter()
        .filter(|e| !matches!(e.action, Ok(AddAction::LoadBehaviourKinds)))
        .filter(|e| {
            let hay = format!("{} {} {}", e.path.join(" "), e.label, e.help).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect()
}

fn add_component_button(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    resources: &SceneResources,
    tools: &mut GameTools,
    id: EntityId,
) {
    let Some(entity) = session.scene.get(id).cloned() else {
        return;
    };
    let filter_id = Id::unique("add-component-filter");
    let mut chosen: Option<AddAction> = None;
    let button = egui::Button::new(egui::RichText::new("      Add component").strong())
        .min_size(vec2(ui.available_width(), 26.0));
    let config = egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let response = egui::containers::menu::MenuButton::from_button(button)
        .config(config)
        .ui(ui, |ui| {
            ui.set_min_width(240.0);
            ui.set_max_width(320.0);
            let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
            let field = ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("Search components…")
                    .desired_width(240.0),
            );
            if !field.has_focus() && filter.is_empty() && !ui.memory(|m| m.has_focus(field.id)) {
                field.request_focus();
            }
            ui.separator();
            let all = entries(session, resources, tools, &entity);
            if filter.trim().is_empty() {
                let refs: Vec<&Entry> = all.iter().collect();
                folder(ui, &refs, 0, &mut chosen);
            } else {
                let found = matching(&all, &filter);
                if found.is_empty() {
                    ui.label(egui::RichText::new("Nothing matches.").weak());
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let mut first = true;
                for e in found {
                    let pick_now = enter && first && e.action.is_ok();
                    if e.action.is_ok() {
                        first = false;
                    }
                    ui.horizontal(|ui| {
                        leaf(ui, e, &mut chosen, pick_now);
                        ui.label(egui::RichText::new(e.path.join(" › ")).small().weak());
                    });
                }
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                ui.close();
            }
            ui.data_mut(|d| d.insert_temp(filter_id, filter));
        });
    let r = &response.0;
    let rect = r.rect;
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 15.0, rect.center().y),
        vec2(14.0, 14.0),
    );
    icons::paint(ui.painter(), icon, Icon::Plus, theme::ACCENT);
    if chosen.is_some() || response.1.is_none() {
        // Start with an empty search next time.
        ui.data_mut(|d| d.remove::<String>(filter_id));
    }
    match chosen {
        Some(AddAction::Run(cmd)) => {
            session.execute(cmd);
        }
        Some(AddAction::LoadBehaviourKinds) => tools.refresh_schema(session),
        None => {}
    }
}

/// Draws the entries at `depth`: sub-folders first (in first-seen order),
/// then the entries filed directly here.
fn folder(ui: &mut egui::Ui, entries: &[&Entry], depth: usize, chosen: &mut Option<AddAction>) {
    let mut names: Vec<&str> = Vec::new();
    for e in entries {
        if let Some(n) = e.path.get(depth)
            && !names.contains(&n.as_str())
        {
            names.push(n);
        }
    }
    for name in names {
        let inside: Vec<&Entry> = entries
            .iter()
            .copied()
            .filter(|e| e.path.get(depth).map(String::as_str) == Some(name))
            .collect();
        let label = if depth == 0 {
            let color = Group::of_folder(name).map_or(theme::TEXT, Group::color);
            egui::RichText::new(name).color(color)
        } else {
            egui::RichText::new(name)
        };
        ui.menu_button(label, |ui| folder(ui, &inside, depth + 1, chosen));
    }
    for e in entries.iter().filter(|e| e.path.len() == depth) {
        leaf(ui, e, chosen, false);
    }
}

fn leaf(ui: &mut egui::Ui, e: &Entry, chosen: &mut Option<AddAction>, pick_now: bool) {
    let enabled = e.action.is_ok();
    let mut r = ui.add_enabled(
        enabled,
        egui::Button::new(&e.label).frame_when_inactive(false),
    );
    if let Err(reason) = &e.action {
        r = r.on_disabled_hover_text(reason);
    } else if !e.help.is_empty() {
        r = r.on_hover_text(&e.help);
    }
    if (r.clicked() || pick_now)
        && let Ok(a) = &e.action
    {
        *chosen = Some(a.clone());
        ui.close();
    }
}

impl Group {
    fn of_folder(name: &str) -> Option<Group> {
        [
            Group::Rendering,
            Group::Lighting,
            Group::Physics,
            Group::Audio,
            Group::Behaviours,
        ]
        .into_iter()
        .find(|g| g.label() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn moving_puts_the_card_before_the_target() {
        let k = keys(&["a", "b", "c", "d"]);
        assert_eq!(moved(&k, 0, 2), keys(&["b", "a", "c", "d"]));
        assert_eq!(moved(&k, 3, 0), keys(&["d", "a", "b", "c"]));
        assert_eq!(moved(&k, 1, 4), keys(&["a", "c", "d", "b"]));
        assert_eq!(moved(&k, 2, 2), k);
        assert_eq!(moved(&k, 2, 3), k);
        assert_eq!(moved(&k, 9, 0), k);
    }

    #[test]
    fn behaviour_titles_are_readable() {
        assert_eq!(behavior_title("psx.camera_zone"), "Camera zone");
        assert_eq!(behavior_title("room.lever"), "Lever");
        assert_eq!(behavior_title("plain"), "Plain");
        assert_eq!(title("behavior:psx.player"), "Player");
        assert_eq!(title("collider"), "Collider");
        assert_eq!(title("rigidbody"), "Rigidbody");
    }

    #[test]
    fn search_needs_every_word() {
        let all = vec![
            Entry::new(&["Rendering", "Mesh"], "Cube", Err(String::new())),
            Entry::new(&["Physics"], "Collider", Err(String::new())),
        ];
        assert_eq!(matching(&all, "mesh cu").len(), 1);
        assert_eq!(matching(&all, "PHYS").len(), 1);
        assert!(matching(&all, "mesh collider").is_empty());
    }

    #[test]
    fn removing_a_behaviour_keeps_the_others() {
        let mut e = Entity::new("e");
        e.behaviors = vec![BehaviorComponent::new("a"), BehaviorComponent::new("b")];
        let Some(Command::SetBehaviors { behaviors, .. }) = remove_command(&e, "behavior:a") else {
            panic!("expected set_behaviors");
        };
        assert_eq!(behaviors, vec![BehaviorComponent::new("b")]);
        assert!(remove_command(&e, "behavior").is_none());
    }
}
