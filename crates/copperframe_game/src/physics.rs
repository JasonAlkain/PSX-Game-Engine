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
//! `copperframe_scene::ColliderComponent`). Changing an entity's scale at runtime
//! does not resize its collider.
//!
//! Events: sensors report overlap changes ([`TriggerEvent`]); colliders of
//! dynamic bodies also report solid contacts starting and stopping
//! ([`ContactEvent`]). Forces added with [`Physics::add_force`] last for one
//! step.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use copperframe_scene::{BodyKind, ColliderComponent, ColliderShape, RigidbodyComponent};
use glam::{Mat4, Quat, Vec3};
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

/// Two solid colliders started (`started`) or stopped touching. At least
/// one of them belongs to a dynamic body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactEvent<E> {
    pub a: E,
    pub b: E,
    pub started: bool,
}

/// What one [`Physics::step`] reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEvents<E> {
    pub triggers: Vec<TriggerEvent<E>>,
    pub contacts: Vec<ContactEvent<E>>,
}

impl<E> Default for StepEvents<E> {
    fn default() -> Self {
        Self {
            triggers: Vec::new(),
            contacts: Vec::new(),
        }
    }
}

/// A shape for [`Physics::overlap`] queries, in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryShape {
    Sphere {
        radius: f32,
    },
    /// Half extents.
    Box {
        half: Vec3,
    },
    /// Vertical capsule; `height` includes the caps.
    Capsule {
        radius: f32,
        height: f32,
    },
}

/// Mass the player character pushes dynamic bodies with (kilograms).
pub const CHARACTER_PUSH_MASS: f32 = 80.0;
/// Default gravity (metres per second squared).
pub const DEFAULT_GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

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
    /// Bodies given a force this step (reset after it).
    forced: Vec<RigidBodyHandle>,
    controller: KinematicCharacterController,
    push_mass: f32,
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
            forced: Vec::new(),
            controller,
            push_mass: CHARACTER_PUSH_MASS,
        }
    }

    pub fn body_count(&self) -> usize {
        self.world.bodies.len()
    }

    pub fn gravity(&self) -> Vec3 {
        gv(self.world.gravity)
    }

    /// Sets the world gravity (non-finite values are ignored).
    pub fn set_gravity(&mut self, gravity: Vec3) {
        if gravity.is_finite() {
            self.world.gravity = rv(gravity);
            self.world.wake_up_all(true);
        }
    }

    /// Mass the character pushes dynamic bodies with; 0 disables pushing.
    pub fn set_character_push_mass(&mut self, mass: f32) {
        if mass.is_finite() && mass >= 0.0 {
            self.push_mass = mass;
        }
    }

    /// Creates the body (and collider, if any) for an entity at `world` (its
    /// world matrix). Without a rigidbody the body is static. `mesh` is
    /// required for [`ColliderShape::Mesh`].
    pub fn add(
        &mut self,
        owner: E,
        collider: Option<&ColliderComponent>,
        rigidbody: Option<&RigidbodyComponent>,
        world: Mat4,
        mesh: Option<&MeshTriangles>,
    ) -> Result<BodyHandle, String> {
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        if !scale.is_finite() || !rotation.is_finite() || !translation.is_finite() {
            return Err("the entity's world transform is not finite".into());
        }
        let kind = BodyKind::of(rigidbody);
        let is_mesh = collider.is_some_and(|c| c.shape == ColliderShape::Mesh);
        if is_mesh && kind != BodyKind::Static {
            return Err("mesh colliders are static only".into());
        }
        let body_pose = if is_mesh {
            Pose::IDENTITY
        } else {
            pose(translation, rotation)
        };
        let mut body = match kind {
            BodyKind::Static => RigidBodyBuilder::fixed(),
            BodyKind::Dynamic => RigidBodyBuilder::dynamic().ccd_enabled(true),
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
        }
        .pose(body_pose);
        if let Some(rb) = rigidbody {
            body = body
                .gravity_scale(rb.gravity_scale)
                .linear_damping(rb.linear_damping)
                .angular_damping(rb.angular_damping);
            if rb.lock_rotation {
                body = body.lock_rotations();
            }
        }
        let Some(c) = collider else {
            let mut body = body;
            if let Some(rb) = rigidbody
                && kind == BodyKind::Dynamic
            {
                // A point mass with the inertia of a 0.5 m ball.
                body = body.additional_mass_properties(MassProperties::new(
                    RVec::ZERO,
                    rb.mass,
                    RVec::splat(0.1 * rb.mass),
                ));
            }
            let body = self.world.insert_body(body);
            return Ok(BodyHandle {
                body,
                collider: ColliderHandle::invalid(),
                kind,
            });
        };
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
            .restitution(c.restitution)
            // The bouncier surface wins, so a bouncy ball bounces on any floor.
            .restitution_combine_rule(CoefficientCombineRule::Max)
            .collision_groups(groups)
            .sensor(c.sensor);
        if !is_mesh {
            builder = builder.translation(rv(c.offset * scale));
        }
        if let Some(rb) = rigidbody
            && kind == BodyKind::Dynamic
        {
            builder = builder.mass(rb.mass);
        }
        if c.sensor {
            // Sensors must notice kinematic (player) and static bodies too.
            builder = builder
                .active_events(ActiveEvents::COLLISION_EVENTS)
                .active_collision_types(ActiveCollisionTypes::all());
        } else if kind == BodyKind::Dynamic {
            builder = builder.active_events(ActiveEvents::COLLISION_EVENTS);
        }
        let (body, collider) = self.world.insert(body, builder);
        self.owners.insert(collider, owner);
        Ok(BodyHandle {
            body,
            collider,
            kind,
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
        self.forced.retain(|b| *b != handle.body);
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

    /// Linear velocity of a dynamic body (m/s); sets it on kinematic bodies
    /// too, though their next target overrides it.
    pub fn set_linear_velocity(&mut self, handle: BodyHandle, v: Vec3) {
        if v.is_finite()
            && let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_dynamic()
        {
            b.set_linvel(rv(v), true);
        }
    }

    /// Angular velocity in radians per second about each world axis.
    pub fn angular_velocity(&self, handle: BodyHandle) -> Vec3 {
        self.world
            .bodies
            .get(handle.body)
            .map_or(Vec3::ZERO, |b| gv(b.angvel()))
    }

    pub fn set_angular_velocity(&mut self, handle: BodyHandle, v: Vec3) {
        if v.is_finite()
            && let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_dynamic()
        {
            b.set_angvel(rv(v), true);
        }
    }

    /// Instantly changes a dynamic body's momentum (newton-seconds): a kick.
    pub fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        if impulse.is_finite()
            && let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_dynamic()
        {
            b.apply_impulse(rv(impulse), true);
        }
    }

    /// Instantly changes a dynamic body's spin.
    pub fn apply_torque_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        if impulse.is_finite()
            && let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_dynamic()
        {
            b.apply_torque_impulse(rv(impulse), true);
        }
    }

    /// Pushes a dynamic body (newtons) during the next step only; call it
    /// every tick for a continuous force.
    pub fn add_force(&mut self, handle: BodyHandle, force: Vec3) {
        if force.is_finite()
            && let Some(b) = self.world.bodies.get_mut(handle.body)
            && b.is_dynamic()
        {
            b.add_force(rv(force), true);
            self.forced.push(handle.body);
        }
    }

    /// Mass of a body in kilograms (0 for static and kinematic bodies).
    pub fn mass(&self, handle: BodyHandle) -> f32 {
        self.world
            .bodies
            .get(handle.body)
            .filter(|b| b.is_dynamic())
            .map_or(0.0, |b| b.mass())
    }

    /// Whether a dynamic body has come to rest and stopped simulating.
    pub fn is_sleeping(&self, handle: BodyHandle) -> bool {
        self.world
            .bodies
            .get(handle.body)
            .is_some_and(|b| b.is_sleeping())
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
        // rapier's collision-only pass consumes the "new body" changes without
        // registering bodies with the island manager, so new dynamic bodies
        // would hang in the air until something touched them. Re-mark them as
        // changed so the next step adds them to the simulation.
        for (_, b) in self.world.bodies.iter_mut() {
            if b.is_dynamic() {
                let p = *b.position();
                b.set_position(p, true);
            }
        }
    }

    /// Advances one fixed step and returns sensor overlap changes and solid
    /// contacts of dynamic bodies.
    pub fn step(&mut self) -> StepEvents<E> {
        self.world.step_with_events(&(), &self.collector);
        for h in self.forced.drain(..) {
            if let Some(b) = self.world.bodies.get_mut(h) {
                b.reset_forces(false);
            }
        }
        let mut out = StepEvents::default();
        while let Ok(event) = self.collisions.try_recv() {
            let (a, b, entered) = match event {
                CollisionEvent::Started(a, b, _) => (a, b, true),
                CollisionEvent::Stopped(a, b, _) => (a, b, false),
            };
            let (Some(&oa), Some(&ob)) = (self.owners.get(&a), self.owners.get(&b)) else {
                // A removed collider.
                continue;
            };
            let sensor = |h| self.world.colliders.get(h).is_some_and(|c| c.is_sensor());
            match (sensor(a), sensor(b)) {
                (true, false) => out.triggers.push(TriggerEvent {
                    sensor: oa,
                    other: ob,
                    entered,
                }),
                (false, true) => out.triggers.push(TriggerEvent {
                    sensor: ob,
                    other: oa,
                    entered,
                }),
                (false, false) => out.contacts.push(ContactEvent {
                    a: oa,
                    b: ob,
                    started: entered,
                }),
                // Sensor-sensor pairs are not reported.
                (true, true) => {}
            }
        }
        out
    }

    /// Every collider overlapping `shape` placed at `center` with `rotation`
    /// (each owner once, in no particular order). Sensors are included only
    /// with `include_sensors`; `exclude`'s body is skipped.
    pub fn overlap(
        &self,
        shape: QueryShape,
        center: Vec3,
        rotation: Quat,
        include_sensors: bool,
        exclude: Option<BodyHandle>,
    ) -> Vec<E> {
        if !center.is_finite() || !rotation.is_finite() {
            return Vec::new();
        }
        let shape: SharedShape = match shape {
            QueryShape::Sphere { radius } if radius.is_finite() && radius > 0.0 => {
                SharedShape::ball(radius)
            }
            QueryShape::Box { half } if half.is_finite() && half.min_element() > 0.0 => {
                SharedShape::cuboid(half.x, half.y, half.z)
            }
            QueryShape::Capsule { radius, height }
                if radius.is_finite() && height.is_finite() && radius > 0.0 && height > 0.0 =>
            {
                SharedShape::capsule_y((height * 0.5 - radius).max(0.0), radius)
            }
            _ => return Vec::new(),
        };
        let mut filter = QueryFilter::default();
        if !include_sensors {
            filter = filter.exclude_sensors();
        }
        if let Some(h) = exclude {
            filter = filter.exclude_rigid_body(h.body);
        }
        let mut out: Vec<E> = Vec::new();
        for (handle, _) in
            self.world
                .intersect_shape(pose(center, rotation.normalize()), &*shape, filter)
        {
            if let Some(&e) = self.owners.get(&handle)
                && !out.contains(&e)
            {
                out.push(e);
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
        let mut collisions = Vec::new();
        let movement = {
            let filter = QueryFilter::exclude_kinematic()
                .exclude_sensors()
                .exclude_rigid_body(ch.handle.body);
            let queries = self.world.query_pipeline_with_filter(filter);
            self.controller
                .move_shape(dt, &queries, &*shape, &(start * offset), rv(desired), |c| {
                    collisions.push(c)
                })
        };
        // Push the dynamic bodies the character ran into.
        if self.push_mass > 0.0 && !collisions.is_empty() {
            let filter = QueryFilter::only_dynamic().exclude_sensors();
            let world = &mut self.world;
            let mut queries = world.broad_phase.as_query_pipeline_mut(
                world.narrow_phase.query_dispatcher(),
                &mut world.bodies,
                &mut world.colliders,
                filter,
            );
            self.controller.solve_character_collision_impulses(
                dt,
                &mut queries,
                &*shape,
                self.push_mass,
                &collisions,
            );
        }
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
        let c = ColliderComponent::new(ColliderShape::Box {
            size: Vec3::new(20.0, 1.0, 20.0),
        });
        p.add(
            0,
            Some(&c),
            None,
            Mat4::from_translation(Vec3::new(0.0, -0.5, 0.0)),
            None,
        )
        .unwrap();
    }

    #[test]
    fn dynamic_box_falls_and_rests_on_the_floor() {
        let mut p = Physics::new(1.0 / 60.0);
        floor(&mut p);
        let c = ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE });
        let h = p
            .add(
                1,
                Some(&c),
                Some(&RigidbodyComponent::dynamic()),
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
        let wall = ColliderComponent::new(ColliderShape::Box {
            size: Vec3::new(1.0, 3.0, 10.0),
        });
        p.add(
            2,
            Some(&wall),
            None,
            Mat4::from_translation(Vec3::new(3.0, 1.5, 0.0)),
            None,
        )
        .unwrap();
        let mut zone = ColliderComponent::new(ColliderShape::Box {
            size: Vec3::new(1.0, 2.0, 1.0),
        });
        zone.sensor = true;
        p.add(
            3,
            Some(&zone),
            None,
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
            for e in p.step().triggers {
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
        let door = ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE });
        let world = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 0.2),
            Quat::IDENTITY,
            Vec3::new(0.0, 1.5, 0.0),
        );
        let h = p
            .add(
                5,
                Some(&door),
                Some(&RigidbodyComponent::kinematic()),
                world,
                None,
            )
            .unwrap();
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
        let door = ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE });
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.8, 2.5, 0.2),
            Quat::IDENTITY,
            Vec3::new(0.0, 1.25, -1.0),
        );
        let h = p
            .add(
                1,
                Some(&door),
                Some(&RigidbodyComponent::kinematic()),
                m,
                None,
            )
            .unwrap();
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

    fn dynamic_box(
        p: &mut Physics<u32>,
        owner: u32,
        at: Vec3,
        c: ColliderComponent,
        rb: RigidbodyComponent,
    ) -> BodyHandle {
        p.add(owner, Some(&c), Some(&rb), Mat4::from_translation(at), None)
            .unwrap()
    }

    #[test]
    fn bounciness_gravity_scale_and_gravity_setting() {
        let mut p: Physics<u32> = Physics::new(1.0 / 60.0);
        floor(&mut p);
        let ball = ColliderComponent {
            restitution: 0.9,
            ..ColliderComponent::new(ColliderShape::Sphere { radius: 0.25 })
        };
        let bouncy = dynamic_box(
            &mut p,
            1,
            Vec3::new(-3.0, 2.0, 0.0),
            ball,
            RigidbodyComponent::dynamic(),
        );
        let floaty = dynamic_box(
            &mut p,
            2,
            Vec3::new(3.0, 2.0, 0.0),
            ColliderComponent::new(ColliderShape::Sphere { radius: 0.25 }),
            RigidbodyComponent {
                gravity_scale: 0.0,
                ..RigidbodyComponent::dynamic()
            },
        );
        let mut max_after_bounce = 0.0f32;
        let mut touched = false;
        let mut contacts = 0;
        for _ in 0..90 {
            let events = p.step();
            contacts += events
                .contacts
                .iter()
                .filter(|c| c.started && (c.a == 1 || c.b == 1))
                .count();
            let y = p.body_pose(bouncy).unwrap().0.y;
            if y < 0.3 {
                touched = true;
            } else if touched {
                max_after_bounce = max_after_bounce.max(y);
            }
        }
        assert!(
            touched && max_after_bounce > 1.0,
            "bounced to {max_after_bounce}"
        );
        assert!(contacts >= 1, "contact with the floor is reported");
        let y = p.body_pose(floaty).unwrap().0.y;
        assert!((y - 2.0).abs() < 1e-3, "gravity scale 0 floats: {y}");

        p.set_gravity(Vec3::new(0.0, 5.0, 0.0));
        assert_eq!(p.gravity(), Vec3::new(0.0, 5.0, 0.0));
        let before = p.body_pose(bouncy).unwrap().0.y;
        for _ in 0..60 {
            p.step();
        }
        assert!(
            p.body_pose(bouncy).unwrap().0.y > before + 1.0,
            "falls upwards"
        );
    }

    #[test]
    fn impulses_forces_velocity_and_overlaps() {
        let mut p: Physics<u32> = Physics::new(1.0 / 60.0);
        p.set_gravity(Vec3::ZERO);
        let rb = RigidbodyComponent {
            mass: 2.0,
            ..RigidbodyComponent::dynamic()
        };
        let b = dynamic_box(
            &mut p,
            1,
            Vec3::ZERO,
            ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }),
            rb,
        );
        assert!((p.mass(b) - 2.0).abs() < 1e-4);
        p.apply_impulse(b, Vec3::new(4.0, 0.0, 0.0));
        let v = p.linear_velocity(b);
        assert!((v.x - 2.0).abs() < 1e-3, "impulse / mass: {v:?}");
        p.set_linear_velocity(b, Vec3::ZERO);
        // A force lasts one step: 120 N on 2 kg for 1/60 s = 1 m/s.
        p.add_force(b, Vec3::new(120.0, 0.0, 0.0));
        p.step();
        p.step();
        let v = p.linear_velocity(b);
        assert!((v.x - 1.0).abs() < 1e-2, "{v:?}");
        p.set_angular_velocity(b, Vec3::new(0.0, 1.0, 0.0));
        assert!((p.angular_velocity(b).y - 1.0).abs() < 1e-4);

        let mut zone = ColliderComponent::new(ColliderShape::Sphere { radius: 0.5 });
        zone.sensor = true;
        p.add(
            2,
            Some(&zone),
            None,
            Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)),
            None,
        )
        .unwrap();
        p.refresh();
        let hits = p.overlap(
            QueryShape::Sphere { radius: 10.0 },
            Vec3::ZERO,
            Quat::IDENTITY,
            false,
            None,
        );
        assert!(hits.contains(&1) && !hits.contains(&2), "{hits:?}");
        let hits = p.overlap(
            QueryShape::Sphere { radius: 10.0 },
            Vec3::ZERO,
            Quat::IDENTITY,
            true,
            Some(b),
        );
        assert_eq!(hits, vec![2]);
        let hits = p.overlap(
            QueryShape::Box {
                half: Vec3::splat(0.1),
            },
            Vec3::new(0.0, 3.0, 0.0),
            Quat::IDENTITY,
            true,
            None,
        );
        assert!(hits.is_empty());
        assert!(
            p.overlap(
                QueryShape::Sphere { radius: -1.0 },
                Vec3::ZERO,
                Quat::IDENTITY,
                true,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn point_mass_without_collider_falls_and_locked_bodies_stay_upright() {
        let mut p: Physics<u32> = Physics::new(1.0 / 60.0);
        floor(&mut p);
        let point = p
            .add(
                1,
                None,
                Some(&RigidbodyComponent::dynamic()),
                Mat4::from_translation(Vec3::Y),
                None,
            )
            .unwrap();
        for _ in 0..30 {
            p.step();
        }
        assert!(
            p.body_pose(point).unwrap().0.y < 0.0,
            "falls through: no collider"
        );

        // A tall box dropped on its corner tips over unless rotation is locked.
        let tilt = Quat::from_rotation_z(0.6);
        let tall = ColliderComponent::new(ColliderShape::Box {
            size: Vec3::new(0.4, 2.0, 0.4),
        });
        let free = p
            .add(
                2,
                Some(&tall),
                Some(&RigidbodyComponent::dynamic()),
                Mat4::from_rotation_translation(tilt, Vec3::new(-3.0, 1.5, 0.0)),
                None,
            )
            .unwrap();
        let locked = p
            .add(
                3,
                Some(&tall),
                Some(&RigidbodyComponent {
                    lock_rotation: true,
                    ..RigidbodyComponent::dynamic()
                }),
                Mat4::from_rotation_translation(tilt, Vec3::new(3.0, 1.5, 0.0)),
                None,
            )
            .unwrap();
        for _ in 0..240 {
            p.step();
        }
        let angle = |h| p.body_pose(h).unwrap().1.angle_between(tilt);
        assert!(angle(free) > 0.3, "free box tipped over");
        assert!(angle(locked) < 1e-3, "locked box kept its rotation");
    }

    #[test]
    fn character_pushes_dynamic_bodies() {
        let dt = 1.0 / 60.0;
        let mut p: Physics<u32> = Physics::new(dt);
        floor(&mut p);
        let crate_ = dynamic_box(
            &mut p,
            1,
            Vec3::new(1.5, 0.5, 0.0),
            ColliderComponent::new(ColliderShape::Box { size: Vec3::ONE }),
            RigidbodyComponent::dynamic(),
        );
        let ch = p.add_character(9, Vec3::ZERO, 0.3, 1.7);
        for _ in 0..30 {
            p.step();
        }
        let start = p.body_pose(crate_).unwrap().0;
        for _ in 0..120 {
            p.move_character(&ch, Vec3::new(2.0 * dt, -0.1 * dt, 0.0), dt);
            p.step();
        }
        let end = p.body_pose(crate_).unwrap().0;
        assert!(end.x > start.x + 0.5, "crate pushed: {start:?} -> {end:?}");

        p.set_character_push_mass(0.0);
        let before = p.body_pose(crate_).unwrap().0;
        for _ in 0..60 {
            p.move_character(&ch, Vec3::new(2.0 * dt, -0.1 * dt, 0.0), dt);
            p.step();
        }
        let after = p.body_pose(crate_).unwrap().0;
        assert!(
            after.x < before.x + 0.1,
            "pushing disabled: {before:?} -> {after:?}"
        );
    }

    #[test]
    fn dynamic_bodies_fall_after_a_refresh() {
        // Regression: a scene load refreshes queries before the first step,
        // which used to leave new dynamic bodies frozen in the air.
        let mut p: Physics<u32> = Physics::new(1.0 / 60.0);
        let h = p
            .add(
                1,
                Some(&ColliderComponent::new(ColliderShape::Box {
                    size: Vec3::ONE,
                })),
                Some(&RigidbodyComponent::dynamic()),
                Mat4::from_translation(Vec3::Y * 2.0),
                None,
            )
            .unwrap();
        p.refresh();
        for _ in 0..30 {
            p.step();
        }
        assert!(p.body_pose(h).unwrap().0.y < 1.0);
    }
}
