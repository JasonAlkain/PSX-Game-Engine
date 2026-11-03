//! Local transforms and the Euler-angle convention used by tools.

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Translation, rotation and scale relative to the parent entity (or the
/// world for root entities). Matrices compose as `T * R * S`.
///
/// In files: `translation` and `scale` are `[x, y, z]`, `rotation` is a unit
/// quaternion `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Smallest absolute scale accepted on any axis. Zero scale cannot be
/// inverted (reparenting and picking need the inverse).
pub const MIN_SCALE: f32 = 1e-4;

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Decomposes an affine matrix. Shear (from non-uniform scale under a
    /// rotated parent) cannot be represented and is lost.
    pub fn from_matrix(m: Mat4) -> Self {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Self {
            translation,
            rotation: rotation.normalize(),
            scale,
        }
    }

    /// Rotation as Euler angles in degrees, `[x, y, z]`. See [`euler_deg_to_quat`].
    pub fn euler_deg(&self) -> Vec3 {
        quat_to_euler_deg(self.rotation)
    }

    /// Describes the first problem that makes this transform unusable.
    pub fn problem(&self) -> Option<String> {
        let all = self
            .translation
            .to_array()
            .into_iter()
            .chain(self.rotation.to_array())
            .chain(self.scale.to_array());
        if !all.into_iter().all(f32::is_finite) {
            return Some("contains a non-finite number".to_owned());
        }
        let len = self.rotation.length();
        if (len - 1.0).abs() > 1e-3 {
            return Some(format!(
                "rotation quaternion has length {len}; it must be a unit quaternion"
            ));
        }
        if self.scale.abs().min_element() < MIN_SCALE {
            return Some(format!(
                "scale {:?} has an axis smaller than {MIN_SCALE}",
                self.scale.to_array()
            ));
        }
        None
    }
}

/// Euler angles in degrees `[x, y, z]` to a quaternion. Rotations apply
/// about Y (yaw) first, then X (pitch), then Z (roll), all intrinsic — the
/// same order as the camera. This is the convention of every
/// `rotation_euler_deg` field and of the editor's rotation fields.
pub fn euler_deg_to_quat(deg: Vec3) -> Quat {
    let r = deg * (std::f32::consts::PI / 180.0);
    Quat::from_euler(EulerRot::YXZ, r.y, r.x, r.z)
}

/// Inverse of [`euler_deg_to_quat`].
pub fn quat_to_euler_deg(q: Quat) -> Vec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_round_trip_and_axis_meaning() {
        let deg = Vec3::new(20.0, -35.0, 10.0);
        let back = quat_to_euler_deg(euler_deg_to_quat(deg));
        assert!(back.abs_diff_eq(deg, 1e-3), "{back:?}");
        // 90° about Y turns -Z (forward) into -X (left), as camera yaw does.
        let f = euler_deg_to_quat(Vec3::new(0.0, 90.0, 0.0)) * Vec3::NEG_Z;
        assert!(f.abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    #[test]
    fn matrix_round_trip() {
        let t = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: euler_deg_to_quat(Vec3::new(10.0, 20.0, 30.0)),
            scale: Vec3::new(2.0, 1.0, 0.5),
        };
        let back = Transform::from_matrix(t.matrix());
        assert!(back.translation.abs_diff_eq(t.translation, 1e-5));
        assert!(back.scale.abs_diff_eq(t.scale, 1e-5));
        assert!(back.rotation.dot(t.rotation).abs() > 0.99999);
    }

    #[test]
    fn problems_are_reported() {
        assert_eq!(Transform::IDENTITY.problem(), None);
        let mut t = Transform::IDENTITY;
        t.scale.y = 0.0;
        assert!(t.problem().unwrap().contains("scale"));
        t = Transform::IDENTITY;
        t.translation.x = f32::NAN;
        assert!(t.problem().unwrap().contains("non-finite"));
        t = Transform::IDENTITY;
        t.rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        assert!(t.problem().unwrap().contains("unit"));
    }
}
