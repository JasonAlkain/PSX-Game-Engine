//! Free-flying debug camera driven by fixed-tick input.

use glam::{Vec2, Vec3};
use psx_render::Camera;
use winit::keyboard::KeyCode;

use crate::input::TickInput;

/// Key bindings. Kept as data so later milestones can route them through
/// named input actions and remapping.
#[derive(Debug, Clone)]
pub struct FlyBindings {
    pub forward: KeyCode,
    pub back: KeyCode,
    pub left: KeyCode,
    pub right: KeyCode,
    pub up: KeyCode,
    pub down: KeyCode,
    pub fast: KeyCode,
}

impl Default for FlyBindings {
    fn default() -> Self {
        Self {
            forward: KeyCode::KeyW,
            back: KeyCode::KeyS,
            left: KeyCode::KeyA,
            right: KeyCode::KeyD,
            up: KeyCode::KeyE,
            down: KeyCode::KeyQ,
            fast: KeyCode::ShiftLeft,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FlyCamera {
    pub bindings: FlyBindings,
    /// Meters per second.
    pub speed: f32,
    pub fast_multiplier: f32,
    /// Radians per unit of raw mouse motion.
    pub mouse_sensitivity: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            bindings: FlyBindings::default(),
            speed: 3.0,
            fast_multiplier: 3.0,
            mouse_sensitivity: 0.0025,
        }
    }
}

impl FlyCamera {
    /// Advances `camera` by one tick of `dt` seconds.
    ///
    /// `look_enabled` gates mouse look (the cursor must be captured).
    pub fn tick(&self, camera: &mut Camera, input: &TickInput, dt: f32, look_enabled: bool) {
        if look_enabled {
            let d: Vec2 = input.mouse_delta * self.mouse_sensitivity;
            // Moving the mouse right turns right (negative yaw); up looks up.
            camera.rotate(-d.x, -d.y);
        }

        let b = &self.bindings;
        let axis = |pos: KeyCode, neg: KeyCode| {
            f32::from(u8::from(input.held(pos))) - f32::from(u8::from(input.held(neg)))
        };
        // Walk on the horizontal plane; E/Q move along world up.
        let forward = Vec3::new(-camera.yaw.sin(), 0.0, -camera.yaw.cos());
        let right = Vec3::new(camera.yaw.cos(), 0.0, -camera.yaw.sin());
        let wish = forward * axis(b.forward, b.back)
            + right * axis(b.right, b.left)
            + Vec3::Y * axis(b.up, b.down);

        let mut speed = self.speed;
        if input.held(b.fast) {
            speed *= self.fast_multiplier;
        }
        camera.position += wish.normalize_or_zero() * speed * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputState;

    #[test]
    fn holding_forward_for_one_second_moves_speed_meters_along_view() {
        let fly = FlyCamera::default();
        let mut cam = Camera::default();
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyW, false);
        for _ in 0..60 {
            fly.tick(&mut cam, &input.begin_tick(), 1.0 / 60.0, false);
        }
        assert!((cam.position - Vec3::new(0.0, 0.0, -fly.speed)).length() < 1e-4);
    }

    #[test]
    fn strafe_follows_camera_right_vector() {
        let fly = FlyCamera::default();
        let mut cam = Camera::default();
        cam.rotate(1.0, 0.3);
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyD, false);
        fly.tick(&mut cam, &input.begin_tick(), 1.0, false);
        let expected = cam.right() * fly.speed;
        assert!((cam.position - expected).length() < 1e-4);
    }

    #[test]
    fn diagonal_movement_is_not_faster() {
        let fly = FlyCamera::default();
        let mut cam = Camera::default();
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyW, false);
        input.key_down(KeyCode::KeyD, false);
        fly.tick(&mut cam, &input.begin_tick(), 1.0, false);
        assert!((cam.position.length() - fly.speed).abs() < 1e-4);
    }

    #[test]
    fn mouse_look_only_when_enabled() {
        let fly = FlyCamera::default();
        let mut input = InputState::default();
        let mut cam = Camera::default();
        input.mouse_moved(Vec2::new(100.0, 0.0));
        fly.tick(&mut cam, &input.begin_tick(), 1.0 / 60.0, false);
        assert_eq!(cam.yaw, 0.0);

        input.mouse_moved(Vec2::new(100.0, 0.0));
        fly.tick(&mut cam, &input.begin_tick(), 1.0 / 60.0, true);
        assert!(cam.right().z > 0.0 || cam.forward().x > 0.0, "turned right");
    }
}
