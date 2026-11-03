//! Physics integration (rapier3d).
//!
//! The only module that touches rapier types. rapier uses glam 0.33
//! (through glamx) while the engine uses glam 0.34, so values cross this
//! boundary as plain components.
//!
//! Transform synchronisation policy:
//!
//! - Static bodies are placed once from the entity's world transform when
//!   the scene loads and never move.
//! - Kinematic bodies follow their entity: before each step the entity's
//!   transform is sent as the body's next kinematic pose.
//! - Dynamic bodies own their pose: after each step the body's pose is
//!   written to the entity (they must be roots, so local = world).
//! - Render interpolation never feeds back into physics, and retro visual
//!   settings never change collision shapes.
//!
//! Collider sizes are scaled by the entity's world scale at creation (see
//! `psx_scene::ColliderComponent`). Changing an entity's scale at runtime
//! does not resize its collider.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use glam::{Mat4, Quat, Vec3};
use psx_scene::{BodyKind, ColliderComponent, ColliderShape};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::*;

type RVec = rapier3d::math::Vector;
type RQuat = rapier3d::math::Rotation;

fn rv(v: Vec3) -> RVec {
    RVec::new(v.x, v.y, v.z)
}
fn gv(v: RVec) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}
fn rq(q: Quat) -> RQuat {
    RQuat::from_xyzw(q.x, q.y, q.z, q.w)
}
fn gq(q: RQuat) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}
fn pose(t: Vec3, r: Quat) -> Pose {
    Pose::from_parts(rv(t), rq(r))
}

/// Physics handles of one entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyHandle {
    pub body: RigidBodyHandle,
    pub collider: ColliderHandle,
    pub kind: BodyKind,
}

/// A character controlled by [`Physics::move_character`] (the player).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterBody {
    pub handle: BodyHandle,
    pub radius: f32,
    pub height: f32,
}

/// An overlap change between a sensor and another collider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerEvent<E> {
    pub sensor: E,
    pub other: E,
    pub entered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit<E> {
    pub entity: E,
    pub distance: f32,
    pub point: Vec3,
    pub normal: Vec3,
}

/// Triangles for a mesh collider, in the entity's local space.
pub struct MeshTriangles {
    pub positions: Vec<Vec3>,
    pub indices: Vec<[u32; 3]>,
}

/// The physics world, generic over the owner key `E` (the runtime uses
/// `hecs::Entity`).
pub struct Physics<E: Copy + Eq + std::hash::Hash> {
    world: PhysicsWorld,
    collector: ChannelEventCollector,
    collisions: Receiver<CollisionEvent>,
    // Kept alive so the collector's other senders stay valid.
    _forces: Receiver<ContactForceEvent>,
    _tears: Receiver<SoftBodyTearEvent>,
    owners: HashMap<ColliderHandle, E>,
    controller: KinematicCharacterController,
}

impl<E: Copy + Eq + std::hash::Hash> Physics<E> {
    /// `dt` is the fixed simulation step in seconds.
    pub fn new(dt: f32) -> Self {
        let (cs, cr) = channel();
        let (fs, fr) = channel();
        let (ts, tr) = channel();
        let mut world = PhysicsWorld::new();
        world.integration_parameters.dt = dt;
        let controller = KinematicCharacterController {
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(0.3),
                min_width: CharacterLength::Absolute(0.2),
                include_dynamic_bodies: false,
            }),
            snap_to_ground: Some(CharacterLength::Absolute(0.2)),
            offset: CharacterLength::Absolute(0.02),
            ..Default::default()
        };
        Self {
            world,
            collector: ChannelEventCollector::new(cs, fs, ts),
            collisions: cr,
            _forces: fr,
            _tears: tr,
            owners: HashMap::new(),
            controller,
        }
    }

    pub fn body_count(&self) -> usize {
        self.world.bodies.len()
    }

    /// Creates the body and collider for an entity at `world` (its world
    /// matrix). `mesh` is required for [`ColliderShape::Mesh`].
    pub fn add(
        &mut self,
        owner: E,
        c: &ColliderComponent,
        world: Mat4,
        mesh: Option<&MeshTriangles>,
    ) -> Result<BodyHandle, String> {
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        if !scale.is_finite() || !rotation.is_finite() || !translation.is_finite() {
            return Err("the entity's world transform is not finite".into());
        }
        let s = scale.abs();
        let builder = match c.shape {
            ColliderShape::Box { size } => {
                let h = size * s * 0.5;
                ColliderBuilder::cuboid(h.x, h.y, h.z)
            }
            ColliderShape::Sphere { radius } => ColliderBuilder::ball(radius * s.max_element()),
            ColliderShape::Capsule { radius, height } => {
                let r = radius * s.x.max(s.z);
                let h = height * s.y;
                // Total height includes the caps; a capsule shorter than its
                // diameter becomes a sphere.
                ColliderBuilder::capsule_y((h * 0.5 - r).max(0.0), r)
            }
            ColliderShape::Mesh => {
                let mesh = mesh.ok_or("a mesh collider needs mesh triangles")?;
                // Bake the full world matrix; the body sits at the origin.
                let vertices = mesh
                    .positions
                    .iter()
                    .map(|p| rv(world.transform_point3(*p)))
                    .collect();
                let mut indices = mesh.indices.clone();
                if world.determinant() < 0.0 {
                    for t in &mut indices {
                        t.swap(1, 2);
                    }
                }
                ColliderBuilder::trimesh(vertices, indices)
                    .map_err(|e| format!("mesh collider: {e}"))?
            }
        };
        let groups = InteractionGroups::new(
            Group::from_bits_retain(1 << c.layer),
            Group::from_bits_retain(u32::from(c.mask)),
            InteractionTestMode::And,
        );
        let mut builder = builder
            .friction(c.friction)
            .collision_groups(groups)
            .sensor(c.sensor);
        if c.shape != ColliderShape::Mesh {
            builder = builder.translation(rv(c.offset * scale));
        }
        if c.body == BodyKind::Dynamic {
            builder = builder.mass(c.mass);
        }
        if c.sensor {
            // Sensors must notice kinematic (player) and static bodies too.
            builder = builder
                .active_events(ActiveEvents::COLLISION_EVENTS)
                .active_collision_types(ActiveCollisionTypes::all());
        }
        let body_pose = if c.shape == ColliderShape::Mesh {
            Pose::IDENTITY
        } else {
            pose(translation, rotation)
        };
        let body = match c.body {
            BodyKind::Static => RigidBodyBuilder::fixed(),
            BodyKind::Dynamic => RigidBodyBuilder::dynamic().ccd_enabled(true),
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
        }
        .pose(body_pose);
        let (body, collider) = self.world.insert(body, builder);
        self.owners.insert(collider, owner);
        Ok(BodyHandle {
            body,
            collider,
            kind: c.body,
        })
    }

    /// A kinematic capsule driven by [`Physics::move_character`]. `feet` is
    /// the bottom of the capsule.
    pub fn add_character(
        &mut self,
        owner: E,
        feet: Vec3,
        radius: f32,
        height: f32,
    ) -> CharacterBody {
        let half = (height * 0.5 - radius).max(0.0);
        let collider = ColliderBuilder::capsule_y(half, radius)
            .translation(rv(Vec3::Y * (height * 0.5)))
            .friction(0.0);
        let body = RigidBodyBuilder::kinematic_position_based().pose(pose(feet, Quat::IDENTITY));
        let (body, collider) = self.world.insert(body, collider);
        self.owners.insert(collider, owner);
        CharacterBody {
            handle: BodyHandle {
                body,
                collider,
                kind: BodyKind::Kinematic,
            },
            radius,
            height,
        }
    }

    pub fn remove(&mut self, handle: BodyHandle) {
        self.owners.remove(&handle.collider);
        self.world.remove_body(handle.body);
    }

    /// Sends the next pose of a kinematic body (applied by the next step).
    pub fn set_kinematic_target(&mut self, handle: BodyHandle, translation: Vec3, rotation: Quat) {
        if let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_kinematic()
        {
            b.set_next_kinematic_position(pose(translation, rotation));
        }
    }

    /// Moves a dynamic body directly (teleport), e.g. on respawn.
    pub fn teleport(&mut self, handle: BodyHandle, translation: Vec3, rotation: Quat) {
        if let Some(b) = self.world.bodies.get_mut(handle.body) {
            b.set_position(pose(translation, rotation), true);
            b.set_linvel(RVec::ZERO, true);
        }
    }

    /// Current pose of a body (translation, rotation).
    pub fn body_pose(&self, handle: BodyHandle) -> Option<(Vec3, Quat)> {
        let b = self.world.bodies.get(handle.body)?;
        let p = b.position();
        Some((gv(p.translation), gq(p.rotation)))
    }

    pub fn linear_velocity(&self, handle: BodyHandle) -> Vec3 {
        self.world
            .bodies
            .get(handle.body)
            .map_or(Vec3::ZERO, |b| gv(b.linvel()))
    }

    /// Updates scene queries after bodies were added or moved without a
    /// step (e.g. right after a scene loads). Overlap events found here are
    /// reported by the next [`Physics::step`].
    pub fn refresh(&mut self) {
        self.world.detect_collisions(&(), &self.collector);
    }

    /// Advances one fixed step and returns sensor overlap changes.
    pub fn step(&mut self) -> Vec<TriggerEvent<E>> {
        self.world.step_with_events(&(), &self.collector);
        let mut out = Vec::new();
        while let Ok(event) = self.collisions.try_recv() {
            let (a, b, entered) = match event {
                CollisionEvent::Started(a, b, _) => (a, b, true),
                CollisionEvent::Stopped(a, b, _) => (a, b, false),
            };
            let sensor = |h| self.world.colliders.get(h).is_some_and(|c| c.is_sensor());
            let (s, o) = match (sensor(a), sensor(b)) {
                (true, false) => (a, b),
                (false, true) => (b, a),
                // Sensor-sensor and removed colliders are not reported.
                _ => continue,
            };
            if let (Some(&sensor), Some(&other)) = (self.owners.get(&s), self.owners.get(&o)) {
                out.push(TriggerEvent {
                    sensor,
                    other,
                    entered,
                });
            }
        }
        out
    }

    /// First solid (non-sensor) collider along a ray, ignoring `exclude`.
    pub fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        exclude: Option<BodyHandle>,
    ) -> Option<RayHit<E>> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO || !origin.is_finite() || !max_distance.is_finite() {
            return None;
        }
        let mut filter = QueryFilter::default().exclude_sensors();
        if let Some(h) = exclude {
            filter = filter.exclude_rigid_body(h.body);
        }
        let ray = Ray::new(rv(origin), rv(dir));
        let (handle, hit) = self
            .world
            .cast_ray_and_get_normal(&ray, max_distance, true, filter)?;
        let entity = *self.owners.get(&handle)?;
        Some(RayHit {
            entity,
            distance: hit.time_of_impact,
            point: origin + dir * hit.time_of_impact,
            normal: gv(hit.normal),
        })
    }

    /// Moves a character by `desired` (meters this step), sliding along
    /// walls, climbing small steps and snapping to the ground. Returns the
    /// new feet position and whether it stands on the ground.
    ///
    /// rapier's controller treats kinematic bodies as moving platforms and
    /// cancels motion away from them while in contact, which makes doors
    /// sticky. So the controller only sees static and dynamic bodies, and
    /// the resulting motion is then swept against kinematic bodies
    /// separately (stop and slide).
    pub fn move_character(&mut self, ch: &CharacterBody, desired: Vec3, dt: f32) -> (Vec3, bool) {
        let Some(body) = self.world.bodies.get(ch.handle.body) else {
            return (Vec3::ZERO, false);
        };
        let start = *body.position();
        let Some(collider) = self.world.colliders.get(ch.handle.collider) else {
            return (gv(start.translation), false);
        };
        let offset = *collider.position_wrt_parent().unwrap_or(&Pose::IDENTITY);
        let shape = collider.shared_shape().clone();
        let movement = {
            let filter = QueryFilter::exclude_kinematic()
                .exclude_sensors()
                .exclude_rigid_body(ch.handle.body);
            let queries = self.world.query_pipeline_with_filter(filter);
            self.controller.move_shape(
                dt,
                &queries,
                &*shape,
                &(start * offset),
                rv(desired),
                |_| {},
            )
        };
        // Sweep against kinematic bodies: up to two contacts (slide once).
        const SKIN: f32 = 0.02;
        let mut feet = gv(start.translation);
        let mut remaining = gv(movement.translation);
        for _ in 0..3 {
            let len = remaining.length();
            if len < 1e-6 {
                break;
            }
            let dir = remaining / len;
            let filter = QueryFilter::only_kinematic()
                .exclude_sensors()
                .exclude_rigid_body(ch.handle.body);
            let shape_pose = Pose::from_parts(rv(feet), RQuat::IDENTITY) * offset;
            let hit = self.world.cast_shape(
                &shape_pose,
                rv(dir),
                &*shape,
                ShapeCastOptions {
                    max_time_of_impact: len + SKIN,
                    target_distance: 0.0,
                    stop_at_penetration: false,
                    compute_impact_geometry_on_penetration: true,
                },
                filter,
            );
            // The obstacle's surface normal, pointing at the character.
            let blocking = hit
                .map(|(_, h)| (h.time_of_impact, gv(h.normal1).normalize_or_zero()))
                .filter(|(_, n)| dir.dot(*n) < -1e-3);
            match blocking {
                Some((toi, normal)) => {
                    let travel = (toi - SKIN).clamp(0.0, len);
                    feet += dir * travel;
                    let mut rest = remaining - dir * travel;
                    let into = rest.dot(normal);
                    if into < 0.0 {
                        rest -= normal * into;
                    }
                    remaining = rest;
                }
                None => {
                    feet += remaining;
                    break;
                }
            }
        }
        if let Some(b) = self.world.bodies.get_mut(ch.handle.body) {
            b.set_next_kinematic_translation(rv(feet));
        }
        (feet, movement.grounded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor(p: &mut Physics<u32>) {
        let c = ColliderComponent::new(
            ColliderShape::Box {
                size: Vec3::new(20.0, 1.0, 20.0),
            },
            BodyKind::Static,
        );
        p.add(
            0,
            &c,
            Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0)),
            None,
        )
        .unwrap();
    }

    #[test]
    fn dynamic_box_falls_and_rests_on_the_floor() {
        let mut p = Physics::new(1.0 / 60.0);
        floor(&mut p);
        let c = ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }, BodyKind::Dynamic);
        let h = p
            .add(
                1,
                &c,
                Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0)),
                None,
            )
            .unwrap();
        for _ in 0..240 {
            p.step();
        }
        let (t, _) = p.body_pose(h).unwrap();
        assert!((t.y - 0.5).abs() < 0.05, "box rests on the floor: {t:?}");
        p.remove(h);
        assert_eq!(p.body_count(), 1);
    }

    #[test]
    fn character_walks_stops_at_walls_and_triggers_sensors() {
        let dt = 1.0 / 60.0;
        let mut p = Physics::new(dt);
        floor(&mut p);
        let wall = ColliderComponent::new(
            ColliderShape::Box {
                size: Vec3::new(1.0, 3.0, 10.0),
            },
            BodyKind::Static,
        );
        p.add(
            2,
            &wall,
            Mat4::from_translation(Vec3::new(3.0, 1.5, 0.0)),
            None,
        )
        .unwrap();
        let mut zone = ColliderComponent::new(
            ColliderShape::Box {
                size: Vec3::new(1.0, 2.0, 1.0),
            },
            BodyKind::Static,
        );
        zone.sensor = true;
        p.add(
            3,
            &zone,
            Mat4::from_translation(Vec3::new(1.5, 1.0, 0.0)),
            None,
        )
        .unwrap();
        let ch = p.add_character(9, Vec3::new(0.0, 0.0, 0.0), 0.3, 1.7);
        let mut entered = false;
        let mut grounded = false;
        let mut feet = Vec3::ZERO;
        for _ in 0..180 {
            // 2 m/s towards +X with gravity.
            let (f, g) = p.move_character(&ch, Vec3::new(2.0 * dt, -9.81 * dt * dt, 0.0), dt);
            feet = f;
            grounded = g;
            for e in p.step() {
                if e.sensor == 3 && e.other == 9 && e.entered {
                    entered = true;
                }
            }
        }
        assert!(grounded);
        assert!(entered, "walking through the zone triggers it");
        // Stopped by the wall at x = 2.5 (minus radius and skin).
        assert!(feet.x < 2.5 - 0.25 && feet.x > 2.0, "{feet:?}");
        assert!(feet.y.abs() < 0.05, "{feet:?}");
        let blocked = feet;
        for _ in 0..30 {
            let (f, _) = p.move_character(&ch, Vec3::new(-2.0 * dt, -0.5 * dt, 0.0), dt);
            feet = f;
            p.step();
        }
        assert!(
            feet.x < blocked.x - 0.9,
            "walks away from the wall: {blocked:?} -> {feet:?}"
        );

        let hit = p
            .raycast(Vec3::new(0.0, 1.0, 0.0), Vec3::X, 10.0, Some(ch.handle))
            .unwrap();
        assert_eq!(hit.entity, 2, "rays ignore sensors and the excluded body");
        assert!((hit.distance - 2.5).abs() < 1e-3);
    }

    #[test]
    fn kinematic_body_follows_its_target_and_scale_sizes_shapes() {
        let mut p = Physics::new(1.0 / 60.0);
        let door =
            ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }, BodyKind::Kinematic);
        let world = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 0.2),
            Quat::IDENTITY,
            Vec3::new(0.0, 1.5, 0.0),
        );
        let h = p.add(5, &door, world, None).unwrap();
        p.refresh();
        // Scaled box: a ray from the side hits the face at x = -1.
        let hit = p
            .raycast(Vec3::new(-5.0, 1.5, 0.0), Vec3::X, 10.0, None)
            .unwrap();
        assert!((hit.distance - 4.0).abs() < 1e-3);
        p.set_kinematic_target(h, Vec3::new(0.0, 4.0, 0.0), Quat::IDENTITY);
        p.step();
        let (t, _) = p.body_pose(h).unwrap();
        assert!((t.y - 4.0).abs() < 1e-4);
        assert!(
            p.raycast(Vec3::new(-5.0, 1.5, 0.0), Vec3::X, 10.0, None)
                .is_none(),
            "the door moved out of the way"
        );
    }

    #[test]
    fn character_is_blocked_by_kinematic_doors_and_can_walk_away() {
        let dt = 1.0 / 60.0;
        let mut p: Physics<u32> = Physics::new(dt);
        floor(&mut p);
        let door =
            ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }, BodyKind::Kinematic);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.8, 2.5, 0.2),
            Quat::IDENTITY,
            Vec3::new(0.0, 1.25, -1.0),
        );
        let h = p.add(1, &door, m, None).unwrap();
        let ch = p.add_character(9, Vec3::new(0.0, 0.0, 1.0), 0.3, 1.75);
        let mut feet = Vec3::ZERO;
        for _ in 0..90 {
            // Walk into the door at an angle: blocked in z, slides in x.
            feet = p.move_character(&ch, Vec3::new(0.5, -0.5, -2.5) * dt, dt).0;
            p.set_kinematic_target(h, Vec3::new(0.0, 1.25, -1.0), Quat::IDENTITY);
            p.step();
        }
        assert!(
            feet.z > -0.62 && feet.z < -0.5,
            "stopped at the door: {feet:?}"
        );
        assert!(feet.x > 0.5, "slid along it: {feet:?}");
        let blocked = feet;
        for _ in 0..30 {
            feet = p.move_character(&ch, Vec3::new(0.0, -0.5, 2.5) * dt, dt).0;
            p.step();
        }
        assert!(
            feet.z > blocked.z + 1.1,
            "walked away: {blocked:?} -> {feet:?}"
        );
    }
}
