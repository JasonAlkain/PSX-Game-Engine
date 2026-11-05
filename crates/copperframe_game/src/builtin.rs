//! Behaviours provided by the engine (`psx.*`), reusable by any game.

use glam::{Vec2, Vec3};

use crate::actions::names;
use crate::api::{Behavior, BehaviorDescriptor, Entity, FieldDescriptor, Params, Registry};
use crate::sim::{CameraState, Ctx};

pub const PLAYER: &str = "psx.player";
pub const CAMERA_ZONE: &str = "psx.camera_zone";
pub const SPIN: &str = "psx.spin";

pub fn register(r: &mut Registry) {
    r.register_in(
        "Engine/Player",
        BehaviorDescriptor {
        kind: PLAYER,
        description: "First-person player: walks with a character capsule (move/run/jump actions), \
                      looks with the mouse, owns the camera and aims interactions. The entity's \
                      position is the player's feet; its yaw is the starting view direction.",
        fields: vec![
            FieldDescriptor::number("speed", 2.5, "walking speed (m/s)"),
            FieldDescriptor::number("run_speed", 4.5, "speed while holding run (m/s)"),
            FieldDescriptor::number(
                "jump_speed",
                0.0,
                "jump take-off speed (m/s); 0 disables jumping",
            ),
            FieldDescriptor::number("eye_height", 1.6, "camera height above the feet (m)"),
            FieldDescriptor::number("radius", 0.3, "capsule radius (m)"),
            FieldDescriptor::number("height", 1.75, "capsule height (m)"),
            FieldDescriptor::number("reach", 2.0, "interaction distance (m)"),
            FieldDescriptor::number("fov", 60.0, "vertical field of view (degrees)"),
        ],
        create: Player::create,
    });
    r.register_in(
        "Engine/Camera",
        BehaviorDescriptor {
        kind: CAMERA_ZONE,
        description: "Fixed camera: while the player is inside this entity's sensor collider, the \
                      view comes from the `camera` entity (looking along its -Z).",
        fields: vec![FieldDescriptor::entity(
            "camera",
            "entity whose transform is the camera; empty = this entity",
        )],
        create: CameraZone::create,
    });
    r.register_in(
        "Engine/Motion",
        BehaviorDescriptor {
            kind: SPIN,
            description: "Rotates the entity about its local Y axis.",
            fields: vec![FieldDescriptor::number(
                "degrees_per_second",
                45.0,
                "rotation speed",
            )],
            create: |p| {
                Ok(Box::new(Spin {
                    rate: p.number("degrees_per_second").to_radians(),
                }))
            },
        },
    );
}

/// Marker component on the player entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerTag;

pub struct Player {
    speed: f32,
    run_speed: f32,
    jump_speed: f32,
    eye_height: f32,
    radius: f32,
    height: f32,
    reach: f32,
    fov: f32,
    yaw: f32,
    pitch: f32,
    vertical: f32,
    grounded: bool,
}

const GRAVITY: f32 = 9.81;

impl Player {
    fn create(p: &Params) -> Result<Box<dyn Behavior>, String> {
        let positive = |k: &str| {
            let v = p.number(k);
            if v > 0.0 && v.is_finite() {
                Ok(v)
            } else {
                Err(format!("{k} must be positive"))
            }
        };
        let radius = positive("radius")?;
        let height = positive("height")?;
        if height < radius * 2.0 {
            return Err("height must be at least twice the radius".into());
        }
        Ok(Box::new(Player {
            speed: positive("speed")?,
            run_speed: positive("run_speed")?,
            jump_speed: p.number("jump_speed").max(0.0),
            eye_height: positive("eye_height")?,
            radius,
            height,
            reach: positive("reach")?,
            fov: p.number("fov").clamp(10.0, 150.0),
            yaw: 0.0,
            pitch: 0.0,
            vertical: 0.0,
            grounded: false,
        }))
    }

    fn eye(&self, feet: Vec3) -> Vec3 {
        feet + Vec3::Y * self.eye_height
    }

    fn camera(&self, feet: Vec3) -> CameraState {
        CameraState {
            position: self.eye(feet),
            yaw: self.yaw,
            pitch: self.pitch,
            fov_deg: self.fov,
        }
    }
}

impl Behavior for Player {
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {
        let world = ctx.world_matrix(me).unwrap_or_default();
        let (_, rotation, feet) = world.to_scale_rotation_translation();
        let f = rotation * Vec3::NEG_Z;
        self.yaw = (-f.x).atan2(-f.z);
        ctx.add_character(me, feet, self.radius, self.height);
        let _ = ctx.world_mut().insert_one(me, PlayerTag);
        // The player moves in world space: detach the rotation/scale from
        // its local transform so it holds just the feet position.
        if let Some(mut local) = ctx.local(me) {
            local.translation = feet;
            local.rotation = glam::Quat::IDENTITY;
            local.scale = Vec3::ONE;
            ctx.set_local(me, local);
        }
        ctx.set_camera(self.camera(feet));
    }

    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        let input = ctx.input().clone();
        let look: Vec2 = input.look;
        self.yaw -= look.x.to_radians();
        self.pitch = (self.pitch + look.y.to_radians()).clamp(-1.45, 1.45);
        let forward = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin());
        let wish = forward * input.axis(names::MOVE_BACK, names::MOVE_FORWARD)
            + right * input.axis(names::MOVE_LEFT, names::MOVE_RIGHT);
        let speed = if input.held(names::RUN) {
            self.run_speed
        } else {
            self.speed
        };
        let horizontal = wish.normalize_or_zero() * speed;
        if self.grounded {
            self.vertical = -0.5; // keep contact on slopes and steps
            if self.jump_speed > 0.0 && input.pressed(names::JUMP) {
                self.vertical = self.jump_speed;
            }
        } else {
            self.vertical = (self.vertical - GRAVITY * ctx.dt()).max(-30.0);
        }
        let desired = (horizontal + Vec3::Y * self.vertical) * ctx.dt();
        if let Some((feet, grounded)) = ctx.move_character(me, desired) {
            self.grounded = grounded;
            if grounded && self.vertical > 0.0 && desired.y <= 0.0 {
                self.vertical = 0.0;
            }
            let cam = self.camera(feet);
            ctx.set_camera(cam);
            ctx.aim(me, cam.position, cam.forward(), self.reach);
        }
    }
}

pub struct CameraZone {
    camera: Option<copperframe_core::EntityId>,
}

impl CameraZone {
    fn create(p: &Params) -> Result<Box<dyn Behavior>, String> {
        Ok(Box::new(CameraZone {
            camera: p.entity("camera"),
        }))
    }
}

impl Behavior for CameraZone {
    fn on_trigger(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, entered: bool) {
        if ctx.world().get::<&PlayerTag>(other).is_err() {
            return;
        }
        let cam = self.camera.and_then(|id| ctx.find(id)).unwrap_or(me);
        if entered {
            ctx.set_camera_override(Some(cam));
        } else if ctx.camera_override() == Some(cam) {
            ctx.set_camera_override(None);
        }
    }
}

struct Spin {
    rate: f32,
}

impl Behavior for Spin {
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        if let Some(mut t) = ctx.local(me) {
            t.rotation =
                (glam::Quat::from_rotation_y(self.rate * ctx.dt()) * t.rotation).normalize();
            ctx.set_local(me, t);
        }
    }
}
