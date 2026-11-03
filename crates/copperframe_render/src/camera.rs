//! Perspective camera and the engine's coordinate conventions.
//!
//! - World space is right-handed, +Y up, units are meters.
//! - A camera looks along its local -Z axis; +X is right.
//! - Matrices are column-major (`glam`), applied as `clip = proj * view * model * v`.
//! - Clip-space depth follows wgpu: `0.0` at the near plane, `1.0` at the far plane.
//! - Yaw rotates about world +Y (positive turns left / counter-clockwise seen
//!   from above); pitch rotates about the camera's +X (positive looks up).

use glam::{Mat4, Quat, Vec3};

/// Pitch is clamped just short of straight up/down to keep the basis well defined.
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// Radians around world +Y. Zero looks down -Z.
    pub yaw: f32,
    /// Radians around the camera's +X. Clamped to `±MAX_PITCH`.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60f32.to_radians(),
            near: 0.05,
            far: 200.0,
        }
    }
}

impl Camera {
    pub fn orientation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }

    /// Unit vector the camera looks along (local -Z in world space).
    pub fn forward(&self) -> Vec3 {
        self.orientation() * Vec3::NEG_Z
    }

    /// Unit vector to the camera's right (local +X in world space).
    pub fn right(&self) -> Vec3 {
        self.orientation() * Vec3::X
    }

    /// Adds to yaw/pitch, wrapping yaw and clamping pitch.
    pub fn rotate(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw = (self.yaw + d_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + d_pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// World-to-view transform.
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.orientation(), self.position).inverse()
    }

    /// View-to-clip transform for the given width/height aspect ratio.
    ///
    /// Non-finite or non-positive aspect ratios (e.g. a minimized window) fall
    /// back to 1.0 so the matrix stays valid.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        // wgpu NDC: right-handed view space in, Y up, depth in [0, 1].
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, self.near, self.far)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }

    /// Linear interpolation between two camera states, for rendering between
    /// fixed simulation ticks. Yaw takes the shortest path.
    pub fn lerp(&self, other: &Camera, t: f32) -> Camera {
        let mut d_yaw = (other.yaw - self.yaw).rem_euclid(std::f32::consts::TAU);
        if d_yaw > std::f32::consts::PI {
            d_yaw -= std::f32::consts::TAU;
        }
        Camera {
            position: self.position.lerp(other.position, t),
            yaw: self.yaw + d_yaw * t,
            pitch: self.pitch + (other.pitch - self.pitch) * t,
            ..*other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn default_camera_looks_down_negative_z_with_x_right() {
        let c = Camera::default();
        assert!(close(c.forward(), Vec3::NEG_Z));
        assert!(close(c.right(), Vec3::X));
    }

    #[test]
    fn positive_yaw_turns_left_and_positive_pitch_looks_up() {
        let mut c = Camera::default();
        c.rotate(std::f32::consts::FRAC_PI_2, 0.0);
        assert!(close(c.forward(), Vec3::NEG_X));
        let mut c = Camera::default();
        c.rotate(0.0, 0.5);
        assert!(c.forward().y > 0.0);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut c = Camera::default();
        c.rotate(0.0, 10.0);
        assert_eq!(c.pitch, MAX_PITCH);
        c.rotate(0.0, -20.0);
        assert_eq!(c.pitch, -MAX_PITCH);
    }

    #[test]
    fn depth_is_zero_at_near_and_one_at_far() {
        let c = Camera::default();
        let vp = c.view_projection(4.0 / 3.0);
        let near = vp * Vec4::new(0.0, 0.0, -c.near, 1.0);
        let far = vp * Vec4::new(0.0, 0.0, -c.far, 1.0);
        assert!((near.z / near.w).abs() < 1e-5);
        assert!((far.z / far.w - 1.0).abs() < 1e-4);
    }

    #[test]
    fn point_right_of_camera_has_positive_clip_x() {
        let c = Camera {
            position: Vec3::new(0.0, 1.0, 5.0),
            ..Camera::default()
        };
        let p = c.view_projection(1.0) * Vec4::new(1.0, 1.0, 0.0, 1.0);
        assert!(p.x / p.w > 0.0);
        assert!(
            p.w > 0.0,
            "point in front of the camera must have positive w"
        );
    }

    #[test]
    fn invalid_aspect_falls_back_to_finite_matrix() {
        let c = Camera::default();
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(c.projection_matrix(aspect).is_finite());
        }
    }

    #[test]
    fn lerp_takes_short_way_around_yaw() {
        let a = Camera {
            yaw: 0.1,
            ..Camera::default()
        };
        let b = Camera {
            yaw: std::f32::consts::TAU - 0.1,
            ..Camera::default()
        };
        let mid = a.lerp(&b, 0.5);
        assert!(mid.yaw.abs() < 1e-5);
    }
}
