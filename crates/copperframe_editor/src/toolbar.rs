//! The bar under the menus: transform tools, snapping, view options and
//! Play/Stop. The Q, W and E keys select the same tools.

use egui::{Align, Color32, DragValue, Layout, RichText};

use copperframe_runtime::scene_gpu::SceneResources;

use crate::dock::Pane;
use crate::gameplay::GameTools;
use crate::gizmo::{Space, Tool, ViewLook};
use crate::icons::{self, Icon};
use crate::session::EditorSession;
use crate::ui::UiState;
use crate::viewport::{EditorCamera, ViewFrame};
use crate::viewport_panel;

pub fn show(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &mut EditorCamera,
    resources: &SceneResources,
    tools: &mut GameTools,
) {
    ui.spacing_mut().item_spacing.x = 3.0;
    ui.horizontal_wrapped(|ui| {
        let prefs = &mut state.prefs;
        for (tool, icon, tip) in [
            (Tool::Move, Icon::Move, "Move (Q): drag an arrow"),
            (Tool::Rotate, Icon::Rotate, "Rotate (W): drag a ring"),
            (
                Tool::Scale,
                Icon::Scale,
                "Scale (E): drag a square, or the centre to scale evenly",
            ),
        ] {
            if icons::button(ui, icon, prefs.tool == tool, tip).clicked() {
                prefs.tool = tool;
            }
        }
        let space = match prefs.space {
            Space::World => "World",
            Space::Local => "Local",
        };
        if ui
            .add_enabled(prefs.tool != Tool::Scale, egui::Button::new(space))
            .on_hover_text("Gizmo axes: the world's or the selection's own")
            .on_disabled_hover_text("Scale always uses the selection's own axes")
            .clicked()
        {
            prefs.space = match prefs.space {
                Space::World => Space::Local,
                Space::Local => Space::World,
            };
        }
        if icons::button(ui, Icon::FrameSelected, false, "Frame the selection (F)").clicked() {
            viewport_panel::frame_selection(session, resources, camera);
        }
        ui.separator();

        if icons::button(
            ui,
            Icon::GridSnap,
            prefs.grid_snap,
            "Grid snap: move, rotate and scale in steps. Ctrl while dragging flips it",
        )
        .clicked()
        {
            prefs.grid_snap = !prefs.grid_snap;
        }
        ui.add(
            DragValue::new(&mut prefs.snap_translate)
                .speed(0.01)
                .range(crate::gizmo::MOVE_STEP_RANGE)
                .suffix(" m"),
        )
        .on_hover_text("Move step ([ halves it, ] doubles it)");
        ui.add(
            DragValue::new(&mut prefs.snap_rotate)
                .speed(0.5)
                .range(1.0..=90.0)
                .suffix("°"),
        )
        .on_hover_text("Rotate step");
        ui.add(
            DragValue::new(&mut prefs.snap_scale)
                .speed(0.01)
                .range(0.01..=10.0)
                .prefix("×"),
        )
        .on_hover_text("Scale step");
        if icons::button(
            ui,
            Icon::Tune,
            state.snap_settings_open,
            "Grid and snap settings",
        )
        .clicked()
        {
            state.snap_settings_open = !state.snap_settings_open;
        }
        let prefs = &mut state.prefs;
        if icons::button(
            ui,
            Icon::VertexSnap,
            prefs.vertex_snap,
            "Vertex snap (hold V): drag a vertex of the selection onto another model's vertex",
        )
        .clicked()
        {
            prefs.vertex_snap = !prefs.vertex_snap;
        }
        if icons::button(
            ui,
            Icon::ModelSnap,
            prefs.model_snap,
            "Model snap: while moving along an axis, line up edges and centres with nearby models",
        )
        .clicked()
        {
            prefs.model_snap = !prefs.model_snap;
        }
        ui.separator();

        egui::ComboBox::from_id_salt("view-frame")
            .width(92.0)
            .selected_text(match prefs.frame {
                ViewFrame::Fill => "Fill panel",
                ViewFrame::Game => "Game frame",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.frame, ViewFrame::Fill, "Fill panel")
                    .on_hover_text("Use all of the panel; the view gets wider");
                ui.selectable_value(&mut prefs.frame, ViewFrame::Game, "Game frame")
                    .on_hover_text("Exactly what the game's window shows, with its bars");
            });
        egui::ComboBox::from_id_salt("view-look")
            .width(70.0)
            .selected_text(match prefs.look {
                ViewLook::Styled => "Styled",
                ViewLook::Sharp => "Sharp",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.look, ViewLook::Styled, "Styled")
                    .on_hover_text("The scene's PS1/PS2 style, as in the game");
                ui.selectable_value(&mut prefs.look, ViewLook::Sharp, "Sharp")
                    .on_hover_text(
                        "Full resolution and steady textures, for precise editing. \
                         The scene's style is not changed",
                    );
            });
        ui.toggle_value(&mut prefs.show_grid, "Grid")
            .on_hover_text("Lines on the grid plane (spacing and height in grid settings)");
        ui.toggle_value(&mut prefs.show_colliders, "Colliders")
            .on_hover_text("Collider wireframes");
        ui.separator();

        if tools.is_playing() {
            if icons::button(ui, Icon::Stop, false, "Stop the game (F5)").clicked() {
                tools.stop(session);
            }
            ui.label(RichText::new("Playing").color(Color32::from_rgb(120, 210, 120)));
        } else if icons::button(
            ui,
            Icon::Play,
            false,
            "Run the game on a copy of this scene (F5)",
        )
        .clicked()
        {
            tools.play(session);
        }
        ui.separator();
        let open = state.dock.is_open(Pane::ProjectSettings);
        if icons::button(
            ui,
            Icon::Cog,
            open,
            "Project settings: name, start scene, window, look, keys, export",
        )
        .clicked()
        {
            state.dock.set_open(Pane::ProjectSettings, !open);
        }
        if let Some(busy) = tools.busy_label() {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(busy).weak());
            });
        }
    });
}
