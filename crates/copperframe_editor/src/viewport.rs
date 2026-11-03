//! Viewport geometry: where the 3D image sits in the window, screen ↔ world
//! conversion for picking and gizmos, and the editor fly camera.
//!
//! All positions here are **physical pixels** relative to the window's
//! top-left corner. egui works in points; the app multiplies by
//! `pixels_per_point` before calling into this module, so DPI scaling is
//! handled in one place.
//!
//! The 3D image is letterboxed inside the viewport panel by the style's fit
//! policy, and rendered at the internal resolution. Picking maps a pixel in
//! the letterboxed image rectangle to normalised device coordinates; the
//! internal resolution does not matter because it has the same aspect
//! ratio as that rectangle. Clicks on the bars pick nothing. Vertex snapping
//! moves drawn vertices by under one internal pixel and is ignored.

use copperframe_core::RetroStyle;
use copperframe_render::{Camera, Rect, ViewLayout};
use copperframe_scene::CameraStart;
use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

/// The viewport panel and the image placed inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportGeometry {
    /// The panel, in window pixels.
    pub panel: Rect,
    /// Internal render size.
    pub internal: (u32, u32),
    /// The image rectangle, in window pixels.
    pub image: Rect,
}

impl ViewportGeometry {
    pub fn new(panel: Rect, style: &RetroStyle) -> Option<Self> {
        let layout = ViewLayout::for_style((panel.width, panel.height), style)?;
        let r = layout.placement.rect;
        Some(Self {
            panel,
            internal: layout.internal,
            image: Rect {
                x: panel.x + r.x,
                y: panel.y + r.y,
                width: r.width,
                height: r.height,
            },
        })
    }

    pub fn aspect(&self) -> f32 {
        self.internal.0 as f32 / self.internal.1.max(1) as f32
    }

    /// NDC for a window pixel, or `None` outside the image (letterbox bars).
    pub fn pixel_to_ndc(&self, px: Vec2) -> Option<Vec2> {
        let r = self.image;
        let local = px - Vec2::new(r.x as f32, r.y as f32);
        if local.x < 0.0 || local.y < 0.0 || local.x > r.width as f32 || local.y > r.height as f32 {
            return None;
        }
        Some(Vec2::new(
            local.x / r.width as f32 * 2.0 - 1.0,
            1.0 - local.y / r.height as f32 * 2.0,
        ))
    }

    /// Window pixel for NDC.
    pub fn ndc_to_pixel(&self, ndc: Vec2) -> Vec2 {
        let r = self.image;
        Vec2::new(
            r.x as f32 + (ndc.x + 1.0) * 0.5 * r.width as f32,
            r.y as f32 + (1.0 - ndc.y) * 0.5 * r.height as f32,
        )
    }

    /// World-space ray (origin on the near plane, unit direction) through a
    /// window pixel.
    pub fn ray(&self, view_proj: Mat4, px: Vec2) -> Option<(Vec3, Vec3)> {
        let ndc = self.pixel_to_ndc(px)?;
        let inv = view_proj.inverse();
        let near = inv * ndc.extend(0.0).extend(1.0);
        let far = inv * ndc.extend(1.0).extend(1.0);
        if near.w.abs() < 1e-12 || far.w.abs() < 1e-12 {
            return None;
        }
        let (a, b) = (near.xyz() / near.w, far.xyz() / far.w);
        let dir = (b - a).try_normalize()?;
        Some((a, dir))
    }

    /// Window pixel of a world point, or `None` if it is behind the camera.
    pub fn project(&self, view_proj: Mat4, p: Vec3) -> Option<Vec2> {
        let clip = view_proj * p.extend(1.0);
        if clip.w <= 1e-4 {
            return None;
        }
        Some(self.ndc_to_pixel(clip.xy() / clip.w))
    }
}

/// The camera used to look at the scene while editing. Hold the right
/// mouse button to look around and fly with WASD/QE; the wheel dollies.
#[derive(Debug, Clone, Copy)]
pub struct EditorCamera {
    pub camera: Camera,
    /// Meters per second.
    pub speed: f32,
}

impl EditorCamera {
    pub fn from_start(start: &CameraStart) -> Self {
        Self {
            camera: Camera {
                position: start.position,
                yaw: start.yaw_deg.to_radians(),
                pitch: start.pitch_deg.to_radians(),
                fov_y: start.fov_deg.to_radians(),
                ..Camera::default()
            },
            speed: 4.0,
        }
    }

    /// The current view as a scene camera start.
    pub fn to_start(&self) -> CameraStart {
        CameraStart {
            position: self.camera.position,
            yaw_deg: self.camera.yaw.to_degrees(),
            pitch_deg: self.camera.pitch.to_degrees(),
            fov_deg: self.camera.fov_y.to_degrees(),
        }
    }

    /// `look` is the mouse delta in points, `motion` is (right, up, forward)
    /// in [-1, 1].
    pub fn update(&mut self, look: Vec2, motion: Vec3, fast: bool, dt: f32) {
        const SENSITIVITY: f32 = 0.004;
        self.camera
            .rotate(-look.x * SENSITIVITY, -look.y * SENSITIVITY);
        let speed = self.speed * if fast { 4.0 } else { 1.0 };
        let c = &mut self.camera;
        let delta = c.right() * motion.x + Vec3::Y * motion.y + c.forward() * motion.z;
        c.position += delta * speed * dt;
    }

    /// A point `distance` meters in front of the camera (where new entities go).
    pub fn spawn_point(&self, distance: f32) -> Vec3 {
        self.camera.position + self.camera.forward() * distance
    }
}

/// Translation gizmo axis handles.
pub const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// Moves `origin` along world `axis` so that its screen projection follows
/// the mouse, given the drag delta in window pixels. Returns the world
/// distance along the axis (unsnapped).
pub fn axis_drag_distance(
    geometry: &ViewportGeometry,
    view_proj: Mat4,
    origin: Vec3,
    axis: Vec3,
    drag_pixels: Vec2,
) -> Option<f32> {
    let a = geometry.project(view_proj, origin)?;
    let b = geometry.project(view_proj, origin + axis)?;
    let screen_axis = b - a;
    let len2 = screen_axis.length_squared();
    if len2 < 1.0 {
        // Axis points (almost) straight at the camera.
        return None;
    }
    Some(drag_pixels.dot(screen_axis) / len2)
}

/// Rounds `v` to a multiple of `step` (no-op for `step <= 0`).
pub fn snap(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> (ViewportGeometry, Mat4, Camera) {
        // A 1000x600 panel at (200, 50): the 4:3 internal image is
        // pillarboxed (integer 2x = 640x480, centred).
        let panel = Rect {
            x: 200,
            y: 50,
            width: 1000,
            height: 600,
        };
        let g = ViewportGeometry::new(panel, &RetroStyle::ps1()).unwrap();
        let cam = Camera {
            position: Vec3::new(0.0, 0.0, 5.0),
            ..Camera::default()
        };
        (g, cam.view_projection(g.aspect()), cam)
    }

    #[test]
    fn letterboxed_image_rect_and_bars() {
        let (g, _, _) = geometry();
        assert_eq!(g.internal, (320, 240));
        assert_eq!(
            g.image,
            Rect {
                x: 200 + 180,
                y: 50 + 60,
                width: 640,
                height: 480
            }
        );
        assert!(
            g.pixel_to_ndc(Vec2::new(250.0, 300.0)).is_none(),
            "left bar"
        );
        let c = g.pixel_to_ndc(Vec2::new(700.0, 350.0)).unwrap();
        assert!(c.abs_diff_eq(Vec2::ZERO, 1e-6));
    }

    #[test]
    fn ray_through_projected_point_hits_it() {
        let (g, vp, cam) = geometry();
        let p = Vec3::new(0.7, -0.4, -1.0);
        let px = g.project(vp, p).unwrap();
        let (o, d) = g.ray(vp, px).unwrap();
        // The point lies on the ray.
        let t = (p - o).dot(d);
        assert!((o + d * t).distance(p) < 1e-3);
        // Centre ray looks along the camera's forward axis.
        let (_, d) = g.ray(vp, Vec2::new(700.0, 350.0)).unwrap();
        assert!(d.abs_diff_eq(cam.forward(), 1e-4));
        assert!(g.project(vp, Vec3::new(0.0, 0.0, 10.0)).is_none(), "behind");
    }

    #[test]
    fn axis_drag_follows_the_mouse() {
        let (g, vp, _) = geometry();
        let origin = Vec3::ZERO;
        let a = g.project(vp, origin).unwrap();
        let b = g.project(vp, origin + Vec3::X).unwrap();
        let d = axis_drag_distance(&g, vp, origin, Vec3::X, (b - a) * 2.0).unwrap();
        assert!((d - 2.0).abs() < 1e-4);
        // Z points at the camera: no reliable screen direction... it still
        // projects to a short segment here, but a pure view-axis gives None.
        let cam_axis = Vec3::Z;
        let far = axis_drag_distance(&g, vp, Vec3::new(0.0, 0.0, -1000.0), cam_axis, Vec2::X);
        assert!(far.is_none());
        assert_eq!(snap(0.37, 0.25), 0.25);
        assert_eq!(snap(0.37, 0.0), 0.37);
    }

    #[test]
    fn camera_start_round_trip() {
        let start = CameraStart {
            position: Vec3::new(1.0, 2.0, 3.0),
            yaw_deg: 30.0,
            pitch_deg: -10.0,
            fov_deg: 70.0,
        };
        let back = EditorCamera::from_start(&start).to_start();
        assert!(back.position.abs_diff_eq(start.position, 1e-6));
        assert!((back.yaw_deg - 30.0).abs() < 1e-4 && (back.pitch_deg + 10.0).abs() < 1e-4);
        assert!((back.fov_deg - 70.0).abs() < 1e-4);
    }
}
