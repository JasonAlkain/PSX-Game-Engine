//! The 3D panel: camera controls, picking, collider and selection overlays,
//! the move, rotate and scale gizmos, snapping, and dropping assets into the
//! scene. The image itself is drawn underneath egui by the app, so this panel
//! paints no background.

use copperframe_core::{AssetId, EntityId};
use copperframe_render::Rect;
use copperframe_runtime::scene_gpu::SceneResources;
use copperframe_scene::{Command, Scene};
use egui::{Color32, Key, Pos2, Sense, Stroke};
use glam::{Mat4, Quat, Vec2, Vec3};
use std::collections::HashMap;

use crate::gameplay;
use crate::gizmo::{
    RingGrab, Tool, axis_scale_factor, gizmo_axes, local_axes, model_snap_offset,
    nearest_on_screen, ray_plane, ring_points, uniform_scale_factor, world_axis_index,
    world_bounds,
};
use crate::session::EditorSession;
use crate::theme;
use crate::ui::UiState;
use crate::viewport::{EditorCamera, ViewportGeometry, axis_drag_distance, snap};

/// An active drag of a gizmo handle.
#[derive(Debug, Clone, Copy)]
pub enum GizmoDrag {
    Move {
        entity: EntityId,
        axis: Vec3,
        start_world: Vec3,
        pixels: Vec2,
    },
    Rotate {
        entity: EntityId,
        axis: Vec3,
        start_rotation: Quat,
        grab: RingGrab,
        pixels: Vec2,
    },
    Scale {
        entity: EntityId,
        /// The local axis, or `None` for the centre handle.
        axis: Option<usize>,
        start_scale: Vec3,
        handle: Vec2,
        pixels: Vec2,
    },
    /// Vertex snapping: `anchor` is the grabbed vertex at the drag's start.
    Vertex {
        entity: EntityId,
        anchor: Vec3,
        start_world: Vec3,
    },
}

/// A grabbable part of the active tool's gizmo.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Handle {
    Axis(usize),
    Uniform,
    /// A rotate ring and the grabbed point on it.
    Ring(usize, Vec3),
}

/// The selection's gizmo as drawn this frame.
struct Gizmo {
    origin: Vec3,
    origin_px: Vec2,
    /// World length of the handles: a constant size on screen.
    size: f32,
    axes: [Vec3; 3],
    /// Screen position of each axis handle's tip.
    tips: [Option<Vec2>; 3],
    /// Each ring's front half, as world points.
    rings: [Vec<Vec3>; 3],
    to_camera: Vec3,
}

const AXIS_COLORS: [Color32; 3] = [
    Color32::from_rgb(232, 84, 72),
    Color32::from_rgb(120, 200, 88),
    Color32::from_rgb(84, 148, 236),
];
/// How close to a handle or vertex the pointer must be, in points.
const GRAB_POINTS: f32 = 8.0;
const VERTEX_POINTS: f32 = 22.0;
const MODEL_SNAP_POINTS: f32 = 10.0;

/// Window pixels for a panel rectangle in points.
pub fn to_pixels(r: egui::Rect, ppp: f32) -> Rect {
    Rect {
        x: (r.min.x * ppp).round().max(0.0) as u32,
        y: (r.min.y * ppp).round().max(0.0) as u32,
        width: (r.width() * ppp).round().max(0.0) as u32,
        height: (r.height() * ppp).round().max(0.0) as u32,
    }
}

/// Draws the panel. Returns its rectangle in points.
pub fn show(
    ui: &mut egui::Ui,
    session: &mut EditorSession,
    state: &mut UiState,
    camera: &mut EditorCamera,
    resources: &SceneResources,
    dt: f32,
) -> Option<egui::Rect> {
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let style = state.prefs.look.apply(&session.scene.settings.style);
    let geometry = ViewportGeometry::new(to_pixels(rect, ppp), &style, state.prefs.frame)?;
    let view_proj = camera.camera.view_projection(geometry.aspect());
    let to_px = |p: Pos2| Vec2::new(p.x * ppp, p.y * ppp);
    let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);

    fly_camera(ui, &response, state, camera, dt);

    let painter = ui.painter_at(rect);
    let world = session.scene.world_matrices();

    if state.prefs.show_grid {
        // Clipped to the image: the bars around a game frame stay clean.
        let image = geometry.image;
        let clip = egui::Rect::from_min_size(
            Pos2::new(image.x as f32 / ppp, image.y as f32 / ppp),
            egui::vec2(image.width as f32 / ppp, image.height as f32 / ppp),
        );
        let on_image = painter.with_clip_rect(clip.intersect(rect));
        draw_grid(&on_image, &geometry, view_proj, camera, ppp);
    }
    if state.prefs.show_colliders {
        draw_colliders(
            &painter, session, resources, &geometry, view_proj, &world, ppp,
        );
    }

    // Selection outline.
    let selected = session
        .selection
        .and_then(|id| world.get(&id).map(|&m| (id, m)));
    if let Some((id, m)) = selected
        && let Some((lo, hi)) = resources.local_bounds(&session.scene, id)
    {
        draw_box(
            &painter,
            &geometry,
            view_proj,
            m,
            lo,
            hi,
            theme::SELECTION,
            1.5,
            ppp,
        );
    }

    let tool = state.prefs.tool;
    let gizmo = selected
        .and_then(|(_, m)| Gizmo::new(state, m, &geometry, view_proj, camera.camera.position));
    let handle_at = |p: Pos2| {
        gizmo
            .as_ref()
            .and_then(|g| g.handle_at(tool, to_px(p), &geometry, view_proj, ppp))
    };

    // Vertex snapping: the toolbar toggle, or V held.
    let vertex_mode = (state.prefs.vertex_snap
        || (ui.input(|i| i.key_down(Key::V)) && !ui.ctx().egui_wants_keyboard_input()))
        && !state.camera_active;
    let selected_vertices = |pointer: Option<Pos2>| -> Option<Vec3> {
        let (id, m) = selected?;
        let verts = vertices_of(resources, &session.scene, id, m);
        nearest_on_screen(
            &geometry,
            view_proj,
            verts,
            to_px(pointer?),
            VERTEX_POINTS * ppp,
        )
    };
    if vertex_mode
        && state.drag.is_none()
        && let Some(v) = selected_vertices(response.hover_pos())
        && let Some(p) = geometry.project(view_proj, v)
    {
        draw_vertex_marker(&painter, to_pos(p), theme::ACCENT);
    }

    // Start a drag. Hit-test where the button went down, not where the
    // pointer is once the drag threshold is passed.
    let press = ui.input(|i| i.pointer.press_origin());
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some((id, m)) = selected
    {
        let anchor = if vertex_mode {
            selected_vertices(press)
        } else {
            None
        };
        state.drag = match (anchor, gizmo.as_ref(), press.and_then(handle_at)) {
            (Some(anchor), _, _) => Some(GizmoDrag::Vertex {
                entity: id,
                anchor,
                start_world: m.w_axis.truncate(),
            }),
            (None, Some(g), Some(handle)) => {
                g.start_drag(tool, id, handle, m, session, &geometry, view_proj)
            }
            _ => None,
        };
    }

    // Draw after a drag may have started so the active handle is highlighted.
    if let Some(g) = &gizmo {
        let hovered = if state.drag.is_some() {
            None
        } else {
            response.hover_pos().and_then(handle_at)
        };
        g.draw(
            tool, &painter, &geometry, view_proj, hovered, state.drag, ppp,
        );
    }

    // Continue the drag.
    if let Some(drag) = state.drag {
        if response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            let delta = Vec2::new(d.x, d.y) * ppp;
            let drag = accumulate(drag, delta);
            state.drag = Some(drag);
            let grid = state.prefs.grid_snap != ui.input(|i| i.modifiers.command);
            let pointer = response.interact_pointer_pos().map(to_px);
            apply_drag(
                drag, session, state, resources, &geometry, view_proj, &world, grid, pointer,
                &painter, ppp,
            );
        }
        if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            state.drag = None;
            session.end_merge();
        }
    } else if response.clicked_by(egui::PointerButton::Primary)
        && let Some(p) = response.interact_pointer_pos()
    {
        // Picking: window pixels -> ray -> nearest mesh triangle.
        session.selection = geometry
            .ray(view_proj, to_px(p))
            .and_then(|(o, d)| resources.pick(&session.scene, &world, o, d))
            .map(|(id, _)| id);
    }

    // A model dragged in from the Assets panel.
    if let Some(asset) = response.dnd_release_payload::<AssetId>() {
        let at = response
            .hover_pos()
            .or_else(|| ui.input(|i| i.pointer.latest_pos()))
            .and_then(|p| drop_point(session, resources, &geometry, view_proj, &world, to_px(p)))
            .unwrap_or_else(|| camera.spawn_point(4.0));
        session.add_model(*asset, at);
    }

    if state.camera_active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    Some(rect)
}

/// Where a dropped model lands: on what is under the pointer, else on the
/// ground plane.
fn drop_point(
    session: &EditorSession,
    resources: &SceneResources,
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    world: &HashMap<EntityId, Mat4>,
    pointer: Vec2,
) -> Option<Vec3> {
    let (o, d) = geometry.ray(view_proj, pointer)?;
    match resources.pick(&session.scene, world, o, d) {
        Some((_, distance)) => Some(o + d * distance),
        None => ray_plane(o, d, Vec3::ZERO, Vec3::Y),
    }
}

/// Right mouse look + WASD/QE; wheel dolly.
fn fly_camera(
    ui: &egui::Ui,
    response: &egui::Response,
    state: &mut UiState,
    camera: &mut EditorCamera,
    dt: f32,
) {
    let looking = response.dragged_by(egui::PointerButton::Secondary);
    state.camera_active = looking;
    if looking {
        let d = response.drag_delta();
        let (motion, fast) = ui.input(|i| {
            let axis = |pos: Key, neg: Key| f32::from(i.key_down(pos)) - f32::from(i.key_down(neg));
            (
                Vec3::new(
                    axis(Key::D, Key::A),
                    axis(Key::E, Key::Q),
                    axis(Key::W, Key::S),
                ),
                i.modifiers.shift,
            )
        });
        camera.update(Vec2::new(d.x, d.y), motion, fast, dt);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            camera.camera.position += camera.camera.forward() * scroll * 0.01;
        }
    }
}

/// Moves the camera so the selection fills the view (F).
pub fn frame_selection(
    session: &EditorSession,
    resources: &SceneResources,
    camera: &mut EditorCamera,
) {
    let Some(id) = session.selection else { return };
    let world = session.scene.world_matrices();
    let Some(&m) = world.get(&id) else { return };
    let (center, radius) = match resources.local_bounds(&session.scene, id) {
        Some((lo, hi)) => {
            let (lo, hi) = world_bounds(m, lo, hi);
            ((lo + hi) * 0.5, (hi - lo).length() * 0.5)
        }
        None => (m.w_axis.truncate(), 1.0),
    };
    let distance = radius.max(0.25) / (camera.camera.fov_y * 0.5).tan().max(0.1) * 1.2;
    camera.camera.position = center - camera.camera.forward() * distance;
}

fn vertices_of(resources: &SceneResources, scene: &Scene, id: EntityId, m: Mat4) -> Vec<Vec3> {
    resources
        .pick_meshes(scene, id)
        .iter()
        .flat_map(|mesh| {
            mesh.positions
                .iter()
                .map(|&p| m.transform_point3(p))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Entities a dragged `id` can snap to: all but itself and its children.
fn snap_targets<'a>(scene: &'a Scene, id: EntityId) -> impl Iterator<Item = EntityId> + 'a {
    scene
        .entities
        .iter()
        .filter(move |e| e.mesh.is_some() && !scene.is_ancestor_or_self(id, e.id))
        .map(|e| e.id)
}

fn accumulate(drag: GizmoDrag, delta: Vec2) -> GizmoDrag {
    let mut drag = drag;
    match &mut drag {
        GizmoDrag::Move { pixels, .. }
        | GizmoDrag::Rotate { pixels, .. }
        | GizmoDrag::Scale { pixels, .. } => *pixels += delta,
        GizmoDrag::Vertex { .. } => {}
    }
    drag
}

fn set_world_translation(
    session: &mut EditorSession,
    world: &HashMap<EntityId, Mat4>,
    entity: EntityId,
    target: Vec3,
) {
    let parent = parent_world(session, world, entity);
    let local = parent.inverse().transform_point3(target);
    session.execute_merging(
        Command::SetTransform {
            id: entity,
            translation: Some(local),
            rotation: None,
            rotation_euler_deg: None,
            scale: None,
        },
        Some(&format!("gizmo:{entity}")),
    );
}

fn parent_world(
    session: &EditorSession,
    world: &HashMap<EntityId, Mat4>,
    entity: EntityId,
) -> Mat4 {
    session
        .scene
        .get(entity)
        .and_then(|e| e.parent)
        .and_then(|p| world.get(&p).copied())
        .unwrap_or(Mat4::IDENTITY)
}

#[allow(clippy::too_many_arguments)]
fn apply_drag(
    drag: GizmoDrag,
    session: &mut EditorSession,
    state: &UiState,
    resources: &SceneResources,
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    world: &HashMap<EntityId, Mat4>,
    grid: bool,
    pointer: Option<Vec2>,
    painter: &egui::Painter,
    ppp: f32,
) {
    let prefs = state.prefs;
    match drag {
        GizmoDrag::Move {
            entity,
            axis,
            start_world,
            pixels,
        } => {
            let Some(dist) = axis_drag_distance(geometry, view_proj, start_world, axis, pixels)
            else {
                return;
            };
            let index = world_axis_index(axis);
            let mut target = start_world + axis * dist;
            if grid {
                match index {
                    // World axes snap to the grid itself, not to steps from
                    // wherever the drag started.
                    Some(i) => target[i] = snap(target[i], prefs.snap_translate),
                    None => target = start_world + axis * snap(dist, prefs.snap_translate),
                }
            }
            // Model snapping takes over when another model is close.
            if prefs.model_snap
                && let Some(i) = index
                && let Some(&m) = world.get(&entity)
                && let Some((lo, hi)) = resources.local_bounds(&session.scene, entity)
            {
                let units = axis_pixels_per_unit(geometry, view_proj, start_world, axis);
                let threshold = MODEL_SNAP_POINTS * ppp / units.max(1.0);
                let (lo, hi) = world_bounds(m, lo, hi);
                let current = m.w_axis.truncate();
                let free = start_world + axis * dist;
                let shift = free[i] - current[i];
                let others: Vec<(Mat4, Vec3, Vec3)> = snap_targets(&session.scene, entity)
                    .filter_map(|id| {
                        let (l, h) = resources.local_bounds(&session.scene, id)?;
                        Some((*world.get(&id)?, l, h))
                    })
                    .collect();
                let boxes: Vec<(Vec3, Vec3)> = others
                    .iter()
                    .map(|&(m, l, h)| world_bounds(m, l, h))
                    .collect();
                let extents: Vec<(f32, f32)> = boxes.iter().map(|b| (b.0[i], b.1[i])).collect();
                if let Some((offset, hit)) =
                    model_snap_offset((lo[i] + shift, hi[i] + shift), &extents, threshold)
                {
                    target = free;
                    target[i] += offset;
                    draw_box(
                        painter,
                        geometry,
                        view_proj,
                        Mat4::IDENTITY,
                        boxes[hit].0,
                        boxes[hit].1,
                        theme::ACCENT,
                        1.5,
                        ppp,
                    );
                }
            }
            set_world_translation(session, world, entity, target);
        }
        GizmoDrag::Rotate {
            entity,
            axis,
            start_rotation,
            grab,
            pixels,
        } => {
            let mut degrees = grab.angle(pixels).to_degrees();
            if grid {
                degrees = snap(degrees, prefs.snap_rotate);
            }
            let (_, parent, _) =
                parent_world(session, world, entity).to_scale_rotation_translation();
            let turned = Quat::from_axis_angle(axis, degrees.to_radians()) * start_rotation;
            session.execute_merging(
                Command::SetTransform {
                    id: entity,
                    translation: None,
                    rotation: Some((parent.inverse() * turned).normalize()),
                    rotation_euler_deg: None,
                    scale: None,
                },
                Some(&format!("gizmo:{entity}")),
            );
        }
        GizmoDrag::Scale {
            entity,
            axis,
            start_scale,
            handle,
            pixels,
        } => {
            let factor = match axis {
                Some(_) => axis_scale_factor(pixels, handle),
                None => uniform_scale_factor(pixels, ppp),
            };
            let scaled = |v: f32| {
                let v = v * factor;
                let v = if grid { snap(v, prefs.snap_scale) } else { v };
                if v.abs() < copperframe_scene::transform::MIN_SCALE {
                    copperframe_scene::transform::MIN_SCALE.copysign(v)
                } else {
                    v
                }
            };
            let mut scale = start_scale;
            match axis {
                Some(i) => scale[i] = scaled(scale[i]),
                None => scale = scale.map(scaled),
            }
            session.execute_merging(
                Command::SetTransform {
                    id: entity,
                    translation: None,
                    rotation: None,
                    rotation_euler_deg: None,
                    scale: Some(scale),
                },
                Some(&format!("gizmo:{entity}")),
            );
        }
        GizmoDrag::Vertex {
            entity,
            anchor,
            start_world,
        } => {
            let Some(cursor) = pointer else { return };
            let targets: Vec<Vec3> = snap_targets(&session.scene, entity)
                .filter_map(|id| Some(vertices_of(resources, &session.scene, id, *world.get(&id)?)))
                .flatten()
                .collect();
            let hit = nearest_on_screen(geometry, view_proj, targets, cursor, VERTEX_POINTS * ppp);
            let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);
            let delta = match hit {
                Some(target) => {
                    if let Some(p) = geometry.project(view_proj, target) {
                        draw_vertex_marker(painter, to_pos(p), theme::ACCENT);
                    }
                    Some(target - anchor)
                }
                // Nothing near: follow the pointer at the vertex's depth.
                None => geometry.ray(view_proj, cursor).and_then(|(o, d)| {
                    let forward = view_proj.inverse().transform_vector3(Vec3::NEG_Z);
                    let normal = forward.try_normalize().unwrap_or(d);
                    ray_plane(o, d, anchor, normal).map(|p| p - anchor)
                }),
            };
            if let Some(delta) = delta {
                set_world_translation(session, world, entity, start_world + delta);
            }
        }
    }
}

fn axis_pixels_per_unit(
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    origin: Vec3,
    axis: Vec3,
) -> f32 {
    match (
        geometry.project(view_proj, origin),
        geometry.project(view_proj, origin + axis),
    ) {
        (Some(a), Some(b)) => a.distance(b),
        _ => 1.0,
    }
}

impl Gizmo {
    fn new(
        state: &UiState,
        m: Mat4,
        geometry: &ViewportGeometry,
        view_proj: Mat4,
        camera: Vec3,
    ) -> Option<Gizmo> {
        let origin = m.w_axis.truncate();
        let origin_px = geometry.project(view_proj, origin)?;
        let size = origin.distance(camera).max(0.1) * 0.15;
        let axes = match state.prefs.tool {
            Tool::Scale => local_axes(m),
            _ => gizmo_axes(state.prefs.space, m),
        };
        let tips = axes.map(|a| geometry.project(view_proj, origin + a * size));
        let to_camera = camera - origin;
        let rings = axes.map(|axis| {
            ring_points(origin, axis, size, 64)
                .into_iter()
                .filter(|p| (*p - origin).dot(to_camera) >= 0.0)
                .collect()
        });
        Some(Gizmo {
            origin,
            origin_px,
            size,
            axes,
            tips,
            rings,
            to_camera,
        })
    }

    fn handle_at(
        &self,
        tool: Tool,
        p: Vec2,
        geometry: &ViewportGeometry,
        view_proj: Mat4,
        ppp: f32,
    ) -> Option<Handle> {
        let reach = GRAB_POINTS * ppp;
        match tool {
            Tool::Scale if self.origin_px.distance(p) < reach => Some(Handle::Uniform),
            Tool::Move | Tool::Scale => self
                .tips
                .iter()
                .enumerate()
                .filter_map(|(i, tip)| Some((i, dist_to_segment(p, self.origin_px, (*tip)?))))
                .filter(|&(_, d)| d < reach)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| Handle::Axis(i)),
            Tool::Rotate => {
                let mut best: Option<(Handle, f32)> = None;
                for (i, ring) in self.rings.iter().enumerate() {
                    for point in ring {
                        let Some(px) = geometry.project(view_proj, *point) else {
                            continue;
                        };
                        let d = px.distance(p);
                        if d < reach && best.is_none_or(|(_, b)| d < b) {
                            best = Some((Handle::Ring(i, *point), d));
                        }
                    }
                }
                best.map(|(h, _)| h)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_drag(
        &self,
        tool: Tool,
        entity: EntityId,
        handle: Handle,
        m: Mat4,
        session: &EditorSession,
        geometry: &ViewportGeometry,
        view_proj: Mat4,
    ) -> Option<GizmoDrag> {
        let (_, rotation, _) = m.to_scale_rotation_translation();
        let start_scale = session.scene.get(entity)?.transform.scale;
        match handle {
            Handle::Ring(i, hit) => Some(GizmoDrag::Rotate {
                entity,
                axis: self.axes[i],
                start_rotation: rotation,
                grab: RingGrab::new(
                    geometry,
                    view_proj,
                    self.origin,
                    self.axes[i],
                    hit,
                    self.size,
                )?,
                pixels: Vec2::ZERO,
            }),
            Handle::Uniform => Some(GizmoDrag::Scale {
                entity,
                axis: None,
                start_scale,
                handle: Vec2::X,
                pixels: Vec2::ZERO,
            }),
            Handle::Axis(i) if tool == Tool::Scale => Some(GizmoDrag::Scale {
                entity,
                axis: Some(i),
                start_scale,
                handle: self.tips[i]? - self.origin_px,
                pixels: Vec2::ZERO,
            }),
            Handle::Axis(i) => Some(GizmoDrag::Move {
                entity,
                axis: self.axes[i],
                start_world: self.origin,
                pixels: Vec2::ZERO,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        tool: Tool,
        painter: &egui::Painter,
        geometry: &ViewportGeometry,
        view_proj: Mat4,
        hovered: Option<Handle>,
        drag: Option<GizmoDrag>,
        ppp: f32,
    ) {
        let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);
        let active_axis = |i: usize| match (hovered, drag) {
            (Some(Handle::Axis(a) | Handle::Ring(a, _)), _) => a == i,
            (_, Some(GizmoDrag::Scale { axis, .. })) => axis == Some(i),
            (_, Some(GizmoDrag::Move { axis, .. } | GizmoDrag::Rotate { axis, .. })) => {
                axis == self.axes[i]
            }
            _ => false,
        };
        match tool {
            Tool::Move | Tool::Scale => {
                for (i, tip) in self.tips.iter().enumerate() {
                    let Some(tip) = *tip else { continue };
                    let active = active_axis(i);
                    let width = if active { 4.0 } else { 2.5 };
                    painter.line_segment(
                        [to_pos(self.origin_px), to_pos(tip)],
                        Stroke::new(width, AXIS_COLORS[i]),
                    );
                    let r = if active { 5.5 } else { 4.5 };
                    if tool == Tool::Move {
                        painter.circle_filled(to_pos(tip), r, AXIS_COLORS[i]);
                    } else {
                        painter.rect_filled(
                            egui::Rect::from_center_size(to_pos(tip), egui::Vec2::splat(r * 2.0)),
                            0.0,
                            AXIS_COLORS[i],
                        );
                    }
                }
                if tool == Tool::Scale {
                    let on = matches!(hovered, Some(Handle::Uniform))
                        || matches!(drag, Some(GizmoDrag::Scale { axis: None, .. }));
                    let side = if on { 14.0 } else { 11.0 };
                    painter.rect_filled(
                        egui::Rect::from_center_size(
                            to_pos(self.origin_px),
                            egui::Vec2::splat(side),
                        ),
                        0.0,
                        theme::ACCENT,
                    );
                }
            }
            Tool::Rotate => {
                for (i, axis) in self.axes.iter().enumerate() {
                    let active = active_axis(i);
                    let points = ring_points(self.origin, *axis, self.size, 64);
                    for (k, point) in points.iter().enumerate() {
                        let next = points[(k + 1) % points.len()];
                        let (Some(a), Some(b)) = (
                            geometry.project(view_proj, *point),
                            geometry.project(view_proj, next),
                        ) else {
                            continue;
                        };
                        // The ring's far half is drawn faintly.
                        let is_front = (*point - self.origin).dot(self.to_camera) >= 0.0;
                        let stroke = match (is_front, active) {
                            (true, true) => Stroke::new(3.5, AXIS_COLORS[i]),
                            (true, false) => Stroke::new(2.0, AXIS_COLORS[i]),
                            (false, _) => Stroke::new(1.0, AXIS_COLORS[i].gamma_multiply(0.3)),
                        };
                        painter.line_segment([to_pos(a), to_pos(b)], stroke);
                    }
                }
            }
        }
    }
}

fn draw_vertex_marker(painter: &egui::Painter, at: Pos2, color: Color32) {
    painter.rect_stroke(
        egui::Rect::from_center_size(at, egui::Vec2::splat(9.0)),
        0.0,
        Stroke::new(2.0, color),
        egui::StrokeKind::Middle,
    );
}

/// Faint lines on the ground plane, one per metre, around the camera.
fn draw_grid(
    painter: &egui::Painter,
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    camera: &EditorCamera,
    ppp: f32,
) {
    const REACH: i32 = 20;
    let (cx, cz) = (
        camera.camera.position.x.round() as i32,
        camera.camera.position.z.round() as i32,
    );
    let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);
    let stroke =
        |major: bool| Stroke::new(1.0, Color32::from_white_alpha(if major { 56 } else { 24 }));
    let segment = |a: Vec3, b: Vec3, major: bool| {
        if let (Some(a), Some(b)) = (
            geometry.project(view_proj, a),
            geometry.project(view_proj, b),
        ) {
            painter.line_segment([to_pos(a), to_pos(b)], stroke(major));
        }
    };
    for line in -REACH..=REACH {
        for step in -REACH..REACH {
            let (x, z) = ((cx + line) as f32, (cz + step) as f32);
            segment(
                Vec3::new(x, 0.0, z),
                Vec3::new(x, 0.0, z + 1.0),
                (cx + line) % 5 == 0,
            );
            let (x, z) = ((cx + step) as f32, (cz + line) as f32);
            segment(
                Vec3::new(x, 0.0, z),
                Vec3::new(x + 1.0, 0.0, z),
                (cz + line) % 5 == 0,
            );
        }
    }
}

/// Collider wireframes (green static, orange dynamic, blue kinematic, yellow
/// sensor).
fn draw_colliders(
    painter: &egui::Painter,
    session: &EditorSession,
    resources: &SceneResources,
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    world: &HashMap<EntityId, Mat4>,
    ppp: f32,
) {
    let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);
    for e in &session.scene.entities {
        let (Some(c), Some(&m)) = (&e.collider, world.get(&e.id)) else {
            continue;
        };
        let bounds = (c.shape == copperframe_scene::ColliderShape::Mesh)
            .then(|| resources.local_bounds(&session.scene, e.id))
            .flatten();
        let width = if session.selection == Some(e.id) {
            1.5
        } else {
            1.0
        };
        let stroke = Stroke::new(width, gameplay::collider_color(c).gamma_multiply(0.8));
        for (a, b) in gameplay::collider_lines(c, m, bounds) {
            if let (Some(pa), Some(pb)) = (
                geometry.project(view_proj, a),
                geometry.project(view_proj, b),
            ) {
                painter.line_segment([to_pos(pa), to_pos(pb)], stroke);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_box(
    painter: &egui::Painter,
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    m: Mat4,
    lo: Vec3,
    hi: Vec3,
    color: Color32,
    width: f32,
    ppp: f32,
) {
    let to_pos = |v: Vec2| Pos2::new(v.x / ppp, v.y / ppp);
    let corners: Vec<Option<Vec2>> = (0..8)
        .map(|i| {
            let c = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            geometry.project(view_proj, m.transform_point3(c))
        })
        .collect();
    let stroke = Stroke::new(width, color);
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let (Some(pa), Some(pb)) = (corners[a], corners[b]) {
            painter.line_segment([to_pos(pa), to_pos(pb)], stroke);
        }
    }
}

fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
