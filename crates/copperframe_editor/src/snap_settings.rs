//! The Grid and Snapping window, opened from the toolbar: snap steps with
//! common presets, how the viewport grid is drawn, and a one-off "snap the
//! selection to the grid".

use copperframe_scene::Command;
use egui::{DragValue, RichText};
use glam::Mat4;

use crate::gizmo::{MOVE_STEP_RANGE, snap_point};
use crate::session::EditorSession;
use crate::ui::UiState;

const MOVE_PRESETS: [f32; 5] = [0.125, 0.25, 0.5, 1.0, 2.0];
const ROTATE_PRESETS: [f32; 4] = [5.0, 15.0, 45.0, 90.0];
const SCALE_PRESETS: [f32; 3] = [0.1, 0.25, 0.5];

/// Shows the window while it is open.
pub fn show(ctx: &egui::Context, session: &mut EditorSession, state: &mut UiState) {
    let mut open = state.snap_settings_open;
    egui::Window::new("Grid and snapping")
        .id(egui::Id::unique("copperframe-snap-settings"))
        .open(&mut open)
        .resizable(false)
        .collapsible(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            let prefs = &mut state.prefs;
            ui.checkbox(
                &mut prefs.grid_snap,
                "Snap while moving, rotating and scaling",
            )
            .on_hover_text("Hold Ctrl while dragging to flip it for one drag");
            ui.add_space(4.0);
            egui::Grid::new("snap-steps")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Move");
                    ui.horizontal(|ui| {
                        ui.add(
                            DragValue::new(&mut prefs.snap_translate)
                                .speed(0.01)
                                .range(MOVE_STEP_RANGE)
                                .suffix(" m"),
                        );
                        for v in MOVE_PRESETS {
                            ui.selectable_value(&mut prefs.snap_translate, v, format!("{v}"));
                        }
                    });
                    ui.end_row();
                    ui.label("Rotate");
                    ui.horizontal(|ui| {
                        ui.add(
                            DragValue::new(&mut prefs.snap_rotate)
                                .speed(0.5)
                                .range(1.0..=90.0)
                                .suffix("°"),
                        );
                        for v in ROTATE_PRESETS {
                            ui.selectable_value(&mut prefs.snap_rotate, v, format!("{v}°"));
                        }
                    });
                    ui.end_row();
                    ui.label("Scale");
                    ui.horizontal(|ui| {
                        ui.add(
                            DragValue::new(&mut prefs.snap_scale)
                                .speed(0.01)
                                .range(0.01..=10.0)
                                .prefix("×"),
                        );
                        for v in SCALE_PRESETS {
                            ui.selectable_value(&mut prefs.snap_scale, v, format!("{v}"));
                        }
                    });
                    ui.end_row();
                });
            ui.label(RichText::new("[ and ] halve and double the move step").weak());

            ui.separator();
            ui.checkbox(&mut prefs.show_grid, "Show the grid");
            let grid = &mut prefs.grid;
            ui.checkbox(&mut grid.follow_move_step, "Grid lines at the move step")
                .on_hover_text("The lines show exactly where snapped moves land");
            egui::Grid::new("grid-look")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Spacing");
                    ui.add_enabled(
                        !grid.follow_move_step,
                        DragValue::new(&mut grid.spacing)
                            .speed(0.01)
                            .range(0.01..=100.0)
                            .suffix(" m"),
                    );
                    ui.end_row();
                    ui.label("Bright line every");
                    ui.add(DragValue::new(&mut grid.major_every).range(1..=100));
                    ui.end_row();
                    ui.label("Height (Y)");
                    ui.add(
                        DragValue::new(&mut grid.height)
                            .speed(0.05)
                            .range(-1000.0..=1000.0)
                            .suffix(" m"),
                    )
                    .on_hover_text("Models dropped on empty space land at this height");
                    ui.end_row();
                    ui.label("Reach");
                    ui.add(
                        DragValue::new(&mut grid.reach)
                            .speed(0.5)
                            .range(1.0..=500.0)
                            .suffix(" m"),
                    );
                    ui.end_row();
                });
            ui.checkbox(&mut grid.snap_placed, "Snap dropped models to the grid")
                .on_hover_text(
                    "With grid snap on, models dragged in from Assets land on a grid point",
                );
            let spacing = prefs.grid_spacing();
            let wanted = if prefs.grid.follow_move_step {
                prefs.snap_translate
            } else {
                prefs.grid.spacing
            };
            if (spacing - wanted).abs() > 1.0e-6 {
                ui.label(
                    RichText::new(format!(
                        "Lines drawn every {spacing} m so the grid stays readable"
                    ))
                    .weak(),
                );
            }

            ui.separator();
            let step = prefs.snap_translate;
            let selected = session.selection;
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(format!("Snap selection to the {step} m grid")),
                )
                .on_disabled_hover_text("Select something first")
                .clicked()
                && let Some(id) = selected
            {
                snap_selection(session, id, step);
            }
        });
    state.snap_settings_open = open;
}

/// Moves `id` to the nearest grid point in world space, as one undo step.
pub fn snap_selection(session: &mut EditorSession, id: copperframe_core::EntityId, step: f32) {
    let world = session.scene.world_matrices();
    let Some(m) = world.get(&id) else {
        return;
    };
    let parent = session
        .scene
        .get(id)
        .and_then(|e| e.parent)
        .and_then(|p| world.get(&p).copied())
        .unwrap_or(Mat4::IDENTITY);
    let target = snap_point(m.w_axis.truncate(), step);
    let local = parent.inverse().transform_point3(target);
    session.execute(Command::SetTransform {
        id,
        translation: Some(local),
        rotation: None,
        rotation_euler_deg: None,
        scale: None,
    });
}
