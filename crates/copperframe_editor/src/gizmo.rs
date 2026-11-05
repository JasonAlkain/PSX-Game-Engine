//! Transform tools: the settings the toolbar edits, and the maths behind the
//! gizmos and snapping. No egui here, so all of it is unit tested.

use copperframe_core::{RetroStyle, TextureFilter};
use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::viewport::{AXES, ViewFrame, ViewportGeometry};

/// The active transform tool (Q, W, E).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tool {
    #[default]
    Move,
    Rotate,
    Scale,
}

/// Which axes the move and rotate gizmos use. Scale is always local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Space {
    #[default]
    World,
    Local,
}

/// How the viewport draws the scene. Only the view changes: the scene's own
/// style is never edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewLook {
    /// The scene's style, as the game shows it.
    #[default]
    Styled,
    /// Native resolution and steady textures, for precise editing.
    Sharp,
}

impl ViewLook {
    /// The style to draw with.
    pub fn apply(self, style: &RetroStyle) -> RetroStyle {
        let mut style = style.clone();
        if self == ViewLook::Sharp {
            style.low_res.enabled = false;
            style.vertex_snap.enabled = false;
            style.affine_uv.enabled = false;
            style.color_reduction.enabled = false;
            style.dither.enabled = false;
            style.textures.filter = TextureFilter::Nearest;
        }
        style
    }
}

/// Toolbar settings, remembered between sessions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolPrefs {
    pub tool: Tool,
    pub space: Space,
    pub grid_snap: bool,
    pub snap_translate: f32,
    pub snap_rotate: f32,
    pub snap_scale: f32,
    pub vertex_snap: bool,
    pub model_snap: bool,
    pub frame: ViewFrame,
    pub look: ViewLook,
    pub show_grid: bool,
    pub show_colliders: bool,
    /// The viewport grid: spacing, height, size and major lines.
    pub grid: GridSettings,
}

/// How the viewport grid is drawn, and how placing models uses it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridSettings {
    /// Draw the grid at the move step, so the lines show where moves land.
    pub follow_move_step: bool,
    /// Line spacing in metres when not following the move step.
    pub spacing: f32,
    /// Every Nth line is brighter. 0 or 1 makes every line the same.
    pub major_every: u32,
    /// Height of the grid plane (Y), in metres. Models dropped on empty
    /// space land on it.
    pub height: f32,
    /// How far the grid reaches from the camera, in metres.
    pub reach: f32,
    /// With grid snap on, models dropped into the viewport land on a grid
    /// point (X and Z).
    pub snap_placed: bool,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            follow_move_step: false,
            spacing: 1.0,
            major_every: 5,
            height: 0.0,
            reach: 20.0,
            snap_placed: true,
        }
    }
}

/// Most lines drawn along each direction, so a fine spacing over a large
/// reach stays cheap.
pub const MAX_GRID_LINES: u32 = 81;

/// Smallest and largest move step the toolbar and settings accept.
pub const MOVE_STEP_RANGE: std::ops::RangeInclusive<f32> = 0.01..=10.0;

impl ToolPrefs {
    /// Grid line spacing in metres, widened as needed so at most
    /// [`MAX_GRID_LINES`] lines are drawn each way.
    pub fn grid_spacing(&self) -> f32 {
        let base = if self.grid.follow_move_step {
            self.snap_translate
        } else {
            self.grid.spacing
        };
        let base = if base.is_finite() && base > 0.0 {
            base
        } else {
            1.0
        };
        let reach = self.grid.reach.clamp(1.0, 500.0);
        let mut spacing = base;
        while 2.0 * reach / spacing > MAX_GRID_LINES as f32 {
            spacing *= 2.0;
        }
        spacing
    }

    /// Halves (`finer`) or doubles the move step, within [`MOVE_STEP_RANGE`].
    pub fn step_move(&mut self, finer: bool) {
        let v = if finer {
            self.snap_translate * 0.5
        } else {
            self.snap_translate * 2.0
        };
        self.snap_translate = v.clamp(*MOVE_STEP_RANGE.start(), *MOVE_STEP_RANGE.end());
    }

    /// Where a model dropped at `at` lands: with grid snap and "snap placed
    /// models" on, X and Z move to the nearest grid point. Y is kept, so a
    /// model dropped on a surface stays on it.
    pub fn place(&self, at: Vec3) -> Vec3 {
        if self.grid_snap && self.grid.snap_placed {
            Vec3::new(
                crate::viewport::snap(at.x, self.snap_translate),
                at.y,
                crate::viewport::snap(at.z, self.snap_translate),
            )
        } else {
            at
        }
    }
}

/// `p` moved to the nearest point of a `step` grid on every axis.
pub fn snap_point(p: Vec3, step: f32) -> Vec3 {
    use crate::viewport::snap;
    Vec3::new(snap(p.x, step), snap(p.y, step), snap(p.z, step))
}

impl Default for ToolPrefs {
    fn default() -> Self {
        Self {
            tool: Tool::Move,
            space: Space::World,
            grid_snap: false,
            snap_translate: 0.25,
            snap_rotate: 15.0,
            snap_scale: 0.1,
            vertex_snap: false,
            model_snap: false,
            frame: ViewFrame::Fill,
            look: ViewLook::Styled,
            show_grid: false,
            show_colliders: true,
            grid: GridSettings::default(),
        }
    }
}

/// The gizmo's unit axes for an entity with this world matrix.
pub fn gizmo_axes(space: Space, world: Mat4) -> [Vec3; 3] {
    match space {
        Space::World => AXES,
        Space::Local => local_axes(world),
    }
}

/// The directions of an entity's own axes in the world.
pub fn local_axes(world: Mat4) -> [Vec3; 3] {
    [
        world.x_axis.truncate().try_normalize().unwrap_or(Vec3::X),
        world.y_axis.truncate().try_normalize().unwrap_or(Vec3::Y),
        world.z_axis.truncate().try_normalize().unwrap_or(Vec3::Z),
    ]
}

/// Evenly spaced points on a circle around `axis`.
pub fn ring_points(origin: Vec3, axis: Vec3, radius: f32, count: usize) -> Vec<Vec3> {
    let (u, v) = axis.any_orthonormal_pair();
    (0..count)
        .map(|i| {
            let a = i as f32 / count as f32 * std::f32::consts::TAU;
            origin + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// The rotate gizmo's drag: where the ring was grabbed, as a direction on
/// screen and a scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingGrab {
    /// Unit direction on screen in which a positive rotation moves the ring.
    pub tangent: Vec2,
    /// Pixels the ring's rim moves per radian.
    pub pixels_per_radian: f32,
}

impl RingGrab {
    /// `hit` is the grabbed point on a ring of radius `radius` around `axis`
    /// through `origin`. `None` if the rim moves straight at the camera there.
    pub fn new(
        geometry: &ViewportGeometry,
        view_proj: Mat4,
        origin: Vec3,
        axis: Vec3,
        hit: Vec3,
        radius: f32,
    ) -> Option<RingGrab> {
        let radial = (hit - origin).try_normalize()?;
        let step = radius * 0.05;
        let a = geometry.project(view_proj, hit)?;
        let b = geometry.project(view_proj, hit + axis.cross(radial) * step)?;
        let screen = b - a;
        let tangent = screen.try_normalize()?;
        Some(RingGrab {
            tangent,
            pixels_per_radian: screen.length() / step * radius,
        })
    }

    /// The rotation in radians for a drag of `pixels`.
    pub fn angle(&self, pixels: Vec2) -> f32 {
        pixels.dot(self.tangent) / self.pixels_per_radian.max(1.0)
    }
}

/// Scale factor for a drag along a scale handle that spans `handle` pixels on
/// screen: dragging the handle's own length doubles the scale.
pub fn axis_scale_factor(pixels: Vec2, handle: Vec2) -> f32 {
    1.0 + pixels.dot(handle) / handle.length_squared().max(1.0)
}

/// Scale factor for the centre handle: right and up grow, left and down
/// shrink, doubling every 100 pixels.
pub fn uniform_scale_factor(pixels: Vec2, pixels_per_point: f32) -> f32 {
    1.0 + (pixels.x - pixels.y) / (100.0 * pixels_per_point)
}

/// World-space box around local bounds.
pub fn world_bounds(world: Mat4, lo: Vec3, hi: Vec3) -> (Vec3, Vec3) {
    let mut out = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let p = world.transform_point3(corner);
        out = (out.0.min(p), out.1.max(p));
    }
    out
}

/// Model snapping along one axis. `moving` is the dragged model's extent on
/// that axis and `others` the extents of the models around it. Lines up any
/// of the moving model's low edge, centre and high edge with any of another
/// model's, if within `threshold`. Returns the correction to add and the
/// index of the model snapped to.
pub fn model_snap_offset(
    moving: (f32, f32),
    others: &[(f32, f32)],
    threshold: f32,
) -> Option<(f32, usize)> {
    let lines = |(lo, hi): (f32, f32)| [lo, (lo + hi) * 0.5, hi];
    let mut best: Option<(f32, usize)> = None;
    for (index, other) in others.iter().enumerate() {
        for from in lines(moving) {
            for to in lines(*other) {
                let offset = to - from;
                if offset.abs() <= threshold && best.is_none_or(|(b, _)| offset.abs() < b.abs()) {
                    best = Some((offset, index));
                }
            }
        }
    }
    best
}

/// The index of the axis `axis` lies along, if it lies along a world axis.
pub fn world_axis_index(axis: Vec3) -> Option<usize> {
    (0..3).find(|&i| axis[i].abs() > 0.999)
}

/// The point whose projection is nearest `cursor`, within `max_pixels`.
pub fn nearest_on_screen(
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    points: impl IntoIterator<Item = Vec3>,
    cursor: Vec2,
    max_pixels: f32,
) -> Option<Vec3> {
    let mut best: Option<(Vec3, f32)> = None;
    for p in points {
        let Some(px) = geometry.project(view_proj, p) else {
            continue;
        };
        let d = px.distance(cursor);
        if d <= max_pixels && best.is_none_or(|(_, b)| d < b) {
            best = Some((p, d));
        }
    }
    best.map(|(p, _)| p)
}

/// Where a ray meets the plane through `point` with normal `normal`.
pub fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-5 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (t >= 0.0).then_some(origin + dir * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copperframe_render::{Camera, Rect};
    use glam::Quat;

    fn view() -> (ViewportGeometry, Mat4) {
        let panel = Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let g = ViewportGeometry::new(panel, &RetroStyle::ps1(), ViewFrame::Fill).unwrap();
        let cam = Camera {
            position: Vec3::new(0.0, 0.0, 5.0),
            ..Camera::default()
        };
        (g, cam.view_projection(g.aspect()))
    }

    #[test]
    fn dragging_along_the_rim_turns_the_ring_the_right_way() {
        let (g, vp) = view();
        // A ring facing the camera (+Z toward it); grabbed at its right side.
        let hit = Vec3::X;
        let grab = RingGrab::new(&g, vp, Vec3::ZERO, Vec3::Z, hit, 1.0).unwrap();
        // Counter-clockwise about +Z moves the right side up the screen.
        assert!(grab.tangent.y < -0.99, "{:?}", grab.tangent);
        // A quarter turn's worth of rim travel is a quarter turn.
        let a = g.project(vp, Vec3::X).unwrap();
        let b = g.project(vp, Vec3::Y).unwrap();
        let arc = a.distance(g.project(vp, Vec3::ZERO).unwrap()) * std::f32::consts::FRAC_PI_2;
        let angle = grab.angle(grab.tangent * arc);
        assert!(
            (angle - std::f32::consts::FRAC_PI_2).abs() < 0.02,
            "{angle}"
        );
        // The rotation really does carry the grabbed point upward.
        let moved = Quat::from_axis_angle(Vec3::Z, angle) * hit;
        assert!(g.project(vp, moved).unwrap().distance(b) < 2.0);
    }

    #[test]
    fn scale_handles_double_at_their_own_length() {
        let handle = Vec2::new(80.0, 0.0);
        assert_eq!(axis_scale_factor(Vec2::ZERO, handle), 1.0);
        assert_eq!(axis_scale_factor(handle, handle), 2.0);
        assert_eq!(axis_scale_factor(handle * -0.5, handle), 0.5);
        // Movement across the handle does nothing.
        assert_eq!(axis_scale_factor(Vec2::new(0.0, 50.0), handle), 1.0);
        assert_eq!(uniform_scale_factor(Vec2::new(100.0, 0.0), 1.0), 2.0);
        assert_eq!(uniform_scale_factor(Vec2::new(0.0, -200.0), 2.0), 2.0);
        assert_eq!(uniform_scale_factor(Vec2::new(0.0, 50.0), 1.0), 0.5);
    }

    #[test]
    fn grid_spacing_follows_the_move_step_and_stays_bounded() {
        let mut prefs = ToolPrefs::default();
        assert_eq!(prefs.grid_spacing(), 1.0);
        prefs.grid.follow_move_step = true;
        prefs.snap_translate = 0.5;
        assert_eq!(prefs.grid_spacing(), 0.5);
        // 0.01 m over 20 m would be 4000 lines; it widens by doubling.
        prefs.snap_translate = 0.01;
        let s = prefs.grid_spacing();
        assert!(2.0 * prefs.grid.reach / s <= MAX_GRID_LINES as f32);
        assert!(2.0 * prefs.grid.reach / (s * 0.5) > MAX_GRID_LINES as f32);
        prefs.grid.follow_move_step = false;
        prefs.grid.spacing = 0.0;
        assert_eq!(prefs.grid_spacing(), 1.0);
    }

    #[test]
    fn move_step_halves_and_doubles_within_range() {
        let mut prefs = ToolPrefs::default();
        prefs.step_move(true);
        assert_eq!(prefs.snap_translate, 0.125);
        prefs.step_move(false);
        prefs.step_move(false);
        assert_eq!(prefs.snap_translate, 0.5);
        prefs.snap_translate = 8.0;
        prefs.step_move(false);
        assert_eq!(prefs.snap_translate, 10.0);
        prefs.snap_translate = 0.015;
        prefs.step_move(true);
        assert_eq!(prefs.snap_translate, 0.01);
    }

    #[test]
    fn placed_models_snap_only_with_grid_snap_on() {
        let mut prefs = ToolPrefs::default();
        let at = Vec3::new(1.13, 0.37, -2.9);
        assert_eq!(prefs.place(at), at);
        prefs.grid_snap = true;
        assert_eq!(prefs.place(at), Vec3::new(1.25, 0.37, -3.0));
        prefs.grid.snap_placed = false;
        assert_eq!(prefs.place(at), at);
        assert_eq!(snap_point(at, 0.5), Vec3::new(1.0, 0.5, -3.0));
    }

    #[test]
    fn older_prefs_without_grid_settings_load() {
        let json = r#"{"tool":"Move","grid_snap":true,"snap_translate":0.5}"#;
        let prefs: ToolPrefs = serde_json::from_str(json).unwrap();
        assert!(prefs.grid_snap);
        assert_eq!(prefs.grid, GridSettings::default());
    }

    #[test]
    fn model_snap_lines_up_edges_and_centres() {
        // Moving box 0..1, a neighbour at 1.1..2: its high edge goes to 1.1.
        let (offset, index) = model_snap_offset((0.0, 1.0), &[(1.1, 2.0)], 0.2).unwrap();
        assert!((offset - 0.1).abs() < 1e-6);
        assert_eq!(index, 0);
        // Centres line up too.
        let (offset, _) = model_snap_offset((0.0, 2.0), &[(1.05, 1.05)], 0.1).unwrap();
        assert!((offset - 0.05).abs() < 1e-6);
        // Nearest of several wins.
        let (_, index) = model_snap_offset((0.0, 1.0), &[(1.2, 3.0), (1.05, 3.0)], 0.5).unwrap();
        assert_eq!(index, 1);
        // Out of range, or nothing around.
        assert_eq!(model_snap_offset((0.0, 1.0), &[(5.0, 6.0)], 0.5), None);
        assert_eq!(model_snap_offset((0.0, 1.0), &[], 0.5), None);
    }

    #[test]
    fn vertex_snap_finds_the_nearest_projected_point() {
        let (g, vp) = view();
        let cursor = g.project(vp, Vec3::new(1.0, 0.0, 0.0)).unwrap() + Vec2::new(3.0, 0.0);
        let points = [Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        assert_eq!(
            nearest_on_screen(&g, vp, points, cursor, 10.0),
            Some(Vec3::new(1.0, 0.0, 0.0))
        );
        assert_eq!(nearest_on_screen(&g, vp, points, cursor, 1.0), None);
        // Behind the camera never matches.
        let behind = [Vec3::new(0.0, 0.0, 50.0)];
        assert_eq!(nearest_on_screen(&g, vp, behind, cursor, 1e6), None);
    }

    #[test]
    fn local_axes_ignore_scale_and_follow_rotation() {
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let m = Mat4::from_scale_rotation_translation(Vec3::splat(3.0), rot, Vec3::ONE);
        let [x, y, z] = gizmo_axes(Space::Local, m);
        assert!(x.abs_diff_eq(Vec3::NEG_Z, 1e-5));
        assert!(y.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(z.abs_diff_eq(Vec3::X, 1e-5));
        assert_eq!(gizmo_axes(Space::World, m), AXES);
        assert_eq!(world_axis_index(Vec3::NEG_Z), Some(2));
        assert_eq!(world_axis_index(Vec3::ONE.normalize()), None);
    }

    #[test]
    fn world_bounds_contain_the_rotated_box() {
        let m = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let (lo, hi) = world_bounds(m, Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
        assert!(lo.abs_diff_eq(Vec3::new(10.0, 0.0, -2.0), 1e-5));
        assert!(hi.abs_diff_eq(Vec3::new(11.0, 1.0, 0.0), 1e-5));
    }

    #[test]
    fn sharp_look_turns_off_only_the_view_effects() {
        let scene = RetroStyle::ps1();
        assert_eq!(ViewLook::Styled.apply(&scene), scene);
        let sharp = ViewLook::Sharp.apply(&scene);
        assert!(!sharp.low_res.enabled && !sharp.affine_uv.enabled && !sharp.vertex_snap.enabled);
        assert_eq!(sharp.fog, scene.fog);
        assert_eq!(sharp.atmosphere, scene.atmosphere);
    }

    #[test]
    fn ray_meets_the_plane_in_front_only() {
        let hit = ray_plane(Vec3::Y, Vec3::NEG_Y, Vec3::ZERO, Vec3::Y).unwrap();
        assert!(hit.abs_diff_eq(Vec3::ZERO, 1e-6));
        assert_eq!(ray_plane(Vec3::Y, Vec3::Y, Vec3::ZERO, Vec3::Y), None);
        assert_eq!(ray_plane(Vec3::Y, Vec3::X, Vec3::ZERO, Vec3::Y), None);
    }

    #[test]
    fn ring_points_are_on_the_circle() {
        for p in ring_points(Vec3::ONE, Vec3::Y, 2.0, 12) {
            assert!(((p - Vec3::ONE).length() - 2.0).abs() < 1e-5);
            assert!((p.y - 1.0).abs() < 1e-5);
        }
    }
}
