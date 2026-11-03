//! Skeletons, animation clips and poses: CPU data and math shared by the
//! importer, the editor preview and the game runtime. No GPU types.
//!
//! A [`Skeleton`] lists joints parent-first with their rest transforms and
//! inverse bind matrices. A [`Pose`] holds one local transform per joint.
//! Clips are sampled into a pose ([`AnimationClip::sample`]), poses can be
//! blended ([`Pose::blend`], used for crossfades), and
//! [`Skeleton::palette`] turns a pose into the joint matrices a skinned
//! mesh is drawn with: `world_joint × inverse_bind`.

use glam::{Mat4, Quat, Vec3, Vec4};

/// Most joints in one skeleton (the renderer's palette size).
pub const MAX_SKIN_JOINTS: usize = 64;

/// Translation, rotation, scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Trs {
    pub const IDENTITY: Trs = Trs {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Component-wise interpolation; rotations take the shortest path.
    pub fn lerp(&self, other: &Trs, t: f32) -> Trs {
        Trs {
            translation: self.translation.lerp(other.translation, t),
            rotation: nlerp(self.rotation, other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

/// Normalised lerp along the shortest arc (stable for nearby keys and
/// cheap; exact enough for keyframe and crossfade blending).
fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    let q = Quat::from_vec4(Vec4::from(a).lerp(Vec4::from(b), t));
    if q.length_squared() > 1.0e-12 {
        q.normalize()
    } else {
        a
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    pub name: String,
    /// Index of the parent joint; always smaller than this joint's index.
    pub parent: Option<usize>,
    /// Local transform in the model's rest pose.
    pub rest: Trs,
    /// For a root joint: the transform above it (import scale and any
    /// non-joint ancestor nodes). Identity for joints with a parent.
    pub base: Mat4,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    /// Parents come before children.
    pub joints: Vec<Joint>,
    /// Mesh space → joint space at bind time, one per joint.
    pub inverse_bind: Vec<Mat4>,
}

/// One local transform per joint of a skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    pub locals: Vec<Trs>,
}

impl Pose {
    /// `a` towards `b` by `t` (0 = `a`). Poses of different lengths blend
    /// over the shorter one and keep `a` for the rest.
    pub fn blend(a: &Pose, b: &Pose, t: f32) -> Pose {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut out = a.clone();
        for (o, bb) in out.locals.iter_mut().zip(&b.locals) {
            *o = o.lerp(bb, t);
        }
        out
    }
}

impl Skeleton {
    pub fn rest_pose(&self) -> Pose {
        Pose {
            locals: self.joints.iter().map(|j| j.rest).collect(),
        }
    }

    /// Model-space joint transforms for `pose`.
    pub fn world(&self, pose: &Pose) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(self.joints.len());
        for (i, joint) in self.joints.iter().enumerate() {
            let local = pose.locals.get(i).unwrap_or(&joint.rest).matrix();
            let parent = joint
                .parent
                .and_then(|p| world.get(p).copied())
                .unwrap_or(joint.base);
            world.push(parent * local);
        }
        world
    }

    /// Skinning matrices for `pose`: `world_joint × inverse_bind`.
    pub fn palette(&self, pose: &Pose) -> Vec<Mat4> {
        self.world(pose)
            .into_iter()
            .zip(&self.inverse_bind)
            .map(|(w, ibm)| w * *ibm)
            .collect()
    }

    pub fn joint_index(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
}

/// Keyframes for one property of one joint.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub joint: usize,
    pub property: ChannelProperty,
    pub interpolation: Interpolation,
    /// Seconds, non-decreasing, at least one key.
    pub times: Vec<f32>,
    /// One value per key: xyz for translation/scale, xyzw for rotation.
    pub values: Vec<Vec4>,
}

impl Channel {
    fn sample(&self, t: f32) -> Vec4 {
        let n = self.times.len().min(self.values.len());
        if n == 0 {
            return Vec4::ZERO;
        }
        // First key after t.
        let next = self.times[..n].partition_point(|&k| k <= t);
        if next == 0 {
            return self.values[0];
        }
        if next >= n {
            return self.values[n - 1];
        }
        let (a, b) = (next - 1, next);
        match self.interpolation {
            Interpolation::Step => self.values[a],
            Interpolation::Linear => {
                let span = self.times[b] - self.times[a];
                let f = if span > 0.0 {
                    ((t - self.times[a]) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                match self.property {
                    ChannelProperty::Rotation => Vec4::from(nlerp(
                        Quat::from_vec4(self.values[a]),
                        Quat::from_vec4(self.values[b]),
                        f,
                    )),
                    _ => self.values[a].lerp(self.values[b], f),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    /// Seconds; the last keyframe time (0 for a single-key pose).
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl AnimationClip {
    /// Local clip time for `elapsed` seconds of playback: wraps when
    /// looping, else clamps to the clip.
    pub fn local_time(&self, elapsed: f32, looping: bool) -> f32 {
        if !elapsed.is_finite() || self.duration <= 0.0 {
            return 0.0;
        }
        if looping {
            elapsed.rem_euclid(self.duration)
        } else {
            elapsed.clamp(0.0, self.duration)
        }
    }

    /// Writes the clip's values at time `t` (seconds, clip-local) into
    /// `pose`. Joints and properties the clip does not animate keep their
    /// values in `pose`.
    pub fn sample_into(&self, pose: &mut Pose, t: f32) {
        for c in &self.channels {
            let Some(local) = pose.locals.get_mut(c.joint) else {
                continue;
            };
            let v = c.sample(t);
            if !v.is_finite() {
                continue;
            }
            match c.property {
                ChannelProperty::Translation => local.translation = v.truncate(),
                ChannelProperty::Rotation => {
                    let q = Quat::from_vec4(v);
                    if q.length_squared() > 1.0e-12 {
                        local.rotation = q.normalize();
                    }
                }
                ChannelProperty::Scale => local.scale = v.truncate(),
            }
        }
    }

    /// The skeleton's rest pose with this clip applied at time `t`.
    pub fn sample(&self, skeleton: &Skeleton, t: f32) -> Pose {
        let mut pose = skeleton.rest_pose();
        self.sample_into(&mut pose, t);
        pose
    }
}

/// Mesh vertex positions deformed by `palette` (linear blend skinning,
/// the same formula as the GPU). Used for picking, bounds and colliders.
pub fn skin_positions(
    positions: impl IntoIterator<Item = ([f32; 3], [u16; 4], [f32; 4])>,
    palette: &[Mat4],
) -> Vec<Vec3> {
    positions
        .into_iter()
        .map(|(p, joints, weights)| {
            let p = Vec3::from(p);
            let mut out = Vec3::ZERO;
            let mut total = 0.0;
            for (j, w) in joints.iter().zip(weights) {
                if w > 0.0
                    && let Some(m) = palette.get(usize::from(*j))
                {
                    out += m.transform_point3(p) * w;
                    total += w;
                }
            }
            if total > 0.0 { out / total } else { p }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two joints: root at the origin, child 1 m up.
    fn arm() -> Skeleton {
        Skeleton {
            joints: vec![
                Joint {
                    name: "root".into(),
                    parent: None,
                    rest: Trs::IDENTITY,
                    base: Mat4::IDENTITY,
                },
                Joint {
                    name: "tip".into(),
                    parent: Some(0),
                    rest: Trs {
                        translation: Vec3::Y,
                        ..Trs::IDENTITY
                    },
                    base: Mat4::IDENTITY,
                },
            ],
            inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)],
        }
    }

    fn swing() -> AnimationClip {
        AnimationClip {
            name: "swing".into(),
            duration: 1.0,
            channels: vec![Channel {
                joint: 0,
                property: ChannelProperty::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![
                    Vec4::from(Quat::IDENTITY),
                    Vec4::from(Quat::from_rotation_z(90f32.to_radians())),
                ],
            }],
        }
    }

    #[test]
    fn rest_palette_is_identity_for_bind_pose() {
        let s = arm();
        for m in s.palette(&s.rest_pose()) {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn sampling_interpolates_and_children_follow_parents() {
        let s = arm();
        let clip = swing();
        let pose = clip.sample(&s, 0.5);
        let world = s.world(&pose);
        // The tip rotates 45° about the root.
        let tip = world[1].w_axis.truncate();
        let expected = Quat::from_rotation_z(45f32.to_radians()) * Vec3::Y;
        assert!(tip.abs_diff_eq(expected, 1e-5), "{tip}");
        // Vertex bound to the tip, 1 m above it in bind space.
        let moved = skin_positions(
            [([0.0, 2.0, 0.0], [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])],
            &s.palette(&pose),
        );
        assert!(moved[0].abs_diff_eq(expected * 2.0, 1e-5));
        // Before the first key / after the last key clamp.
        assert_eq!(clip.sample(&s, -1.0), clip.sample(&s, 0.0));
        assert_eq!(clip.sample(&s, 5.0), clip.sample(&s, 1.0));
    }

    #[test]
    fn looping_wraps_and_one_shots_clamp() {
        let clip = swing();
        assert!((clip.local_time(2.25, true) - 0.25).abs() < 1e-6);
        assert_eq!(clip.local_time(2.25, false), 1.0);
        assert_eq!(clip.local_time(f32::NAN, true), 0.0);
        let still = AnimationClip {
            duration: 0.0,
            ..clip
        };
        assert_eq!(still.local_time(3.0, true), 0.0);
    }

    #[test]
    fn blending_crossfades_and_step_holds() {
        let s = arm();
        let a = s.rest_pose();
        let b = swing().sample(&s, 1.0);
        let half = Pose::blend(&a, &b, 0.5);
        let angle = half.locals[0].rotation.to_axis_angle().1;
        assert!((angle - 45f32.to_radians()).abs() < 1e-4);
        assert_eq!(Pose::blend(&a, &b, 0.0), a);
        assert_eq!(Pose::blend(&a, &b, f32::NAN), a);

        let mut step = swing();
        step.channels[0].interpolation = Interpolation::Step;
        assert_eq!(step.sample(&s, 0.99), a);
    }
}
