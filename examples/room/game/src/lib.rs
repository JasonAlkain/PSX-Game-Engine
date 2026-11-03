//! "The Lever Room": find the lever that opens the door, follow the
//! humming corridor, and reach the exit.
//!
//! Behaviour kinds (parameters are edited in the editor's inspector):
//! - `room.door`: slides by (`slide_x`, `slide_y`, `slide_z`) metres at
//!   `speed` m/s when it receives an `open` message. Plays its audio source.
//! - `room.lever`: interactive; sends `open` to `target` once.
//! - `room.inspect`: interactive; shows `text`.
//! - `room.exit`: when the player enters its sensor, the game is won.
//! - `room.caretaker`: interactive character; crossfades from its looping
//!   `idle` clip to `wave`, says `text`, and fades back to `idle` when the
//!   wave ends.

use glam::{Quat, Vec3};
use psx_game::builtin::PlayerTag;
use psx_game::{
    Behavior, BehaviorDescriptor, Ctx, Entity, FieldDescriptor, Game, GameEvent, Params, Registry,
};

pub struct LeverRoom;

pub const DOOR: &str = "room.door";
pub const LEVER: &str = "room.lever";
pub const INSPECT: &str = "room.inspect";
pub const EXIT: &str = "room.exit";
pub const CARETAKER: &str = "room.caretaker";
/// Game-state key set once the door has been opened (kept in saves).
pub const DOOR_OPEN: &str = "door_open";

impl Game for LeverRoom {
    fn title(&self) -> String {
        "The Lever Room".into()
    }

    fn register(&self, r: &mut Registry) {
        r.register(BehaviorDescriptor {
            kind: DOOR,
            description: "Slides open when it receives an `open` message and plays its sound.",
            fields: vec![
                FieldDescriptor::number("slide_x", 2.0, "opening offset X (m)"),
                FieldDescriptor::number("slide_y", 0.0, "opening offset Y (m)"),
                FieldDescriptor::number("slide_z", 0.0, "opening offset Z (m)"),
                FieldDescriptor::number("speed", 1.5, "opening speed (m/s)"),
            ],
            create: Door::create,
        });
        r.register(BehaviorDescriptor {
            kind: LEVER,
            description: "Interactive lever: sends `open` to its target once and plays its sound.",
            fields: vec![
                FieldDescriptor::entity("target", "entity that receives `open`"),
                FieldDescriptor::text("prompt", "Pull lever", "interaction prompt"),
            ],
            create: |p| {
                Ok(Box::new(Lever {
                    target: p.entity("target"),
                    prompt: p.text("prompt").to_owned(),
                    pulled: false,
                }))
            },
        });
        r.register(BehaviorDescriptor {
            kind: INSPECT,
            description: "Interactive object that shows a line of text.",
            fields: vec![
                FieldDescriptor::text("prompt", "Inspect", "interaction prompt"),
                FieldDescriptor::text("text", "", "text shown when inspected"),
                FieldDescriptor::number("seconds", 5.0, "how long the text stays"),
            ],
            create: |p| {
                Ok(Box::new(Inspect {
                    prompt: p.text("prompt").to_owned(),
                    text: p.text("text").to_owned(),
                    seconds: p.number("seconds").clamp(0.5, 60.0),
                }))
            },
        });
        r.register(BehaviorDescriptor {
            kind: EXIT,
            description: "Sensor: the player entering it wins the game.",
            fields: vec![FieldDescriptor::text(
                "message",
                "You escaped!",
                "shown on the end screen",
            )],
            create: |p| {
                Ok(Box::new(Exit {
                    message: p.text("message").to_owned(),
                }))
            },
        });
        r.register(BehaviorDescriptor {
            kind: CARETAKER,
            description: "Animated character: waves (crossfading from idle) and talks when used.",
            fields: vec![
                FieldDescriptor::text("prompt", "Talk", "interaction prompt"),
                FieldDescriptor::text("text", "", "what the character says"),
                FieldDescriptor::text("idle_clip", "idle", "looping clip between talks"),
                FieldDescriptor::text("talk_clip", "wave", "clip played when used"),
                FieldDescriptor::number("fade", 0.25, "crossfade length (s)"),
            ],
            create: |p| {
                Ok(Box::new(Caretaker {
                    prompt: p.text("prompt").to_owned(),
                    text: p.text("text").to_owned(),
                    idle: p.text("idle_clip").to_owned(),
                    talk: p.text("talk_clip").to_owned(),
                    fade: p.number("fade").clamp(0.0, 2.0),
                    talking: false,
                }))
            },
        });
    }

    fn scene_started(&mut self, ctx: &mut Ctx) {
        ctx.show_message(
            "Find a way out.   WASD move · mouse look · E interact · Esc pause",
            6.0,
        );
    }
}

struct Door {
    offset: Vec3,
    speed: f32,
    closed_at: Option<Vec3>,
    /// 0 = closed, 1 = open.
    progress: f32,
    opening: bool,
}

impl Door {
    fn create(p: &Params) -> Result<Box<dyn Behavior>, String> {
        let speed = p.number("speed");
        if !(speed.is_finite() && speed > 0.0) {
            return Err("speed must be positive".into());
        }
        Ok(Box::new(Door {
            offset: Vec3::new(
                p.number("slide_x"),
                p.number("slide_y"),
                p.number("slide_z"),
            ),
            speed,
            closed_at: None,
            progress: 0.0,
            opening: false,
        }))
    }

    fn place(&self, ctx: &mut Ctx, me: Entity) {
        if let (Some(base), Some(mut t)) = (self.closed_at, ctx.local(me)) {
            t.translation = base + self.offset * self.progress;
            ctx.set_local(me, t);
        }
    }
}

impl Behavior for Door {
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {
        self.closed_at = ctx.local(me).map(|t| t.translation);
        // A saved game resumes with the door already open.
        if ctx.state().get(DOOR_OPEN) == Some(&true.into()) {
            self.progress = 1.0;
            self.place(ctx, me);
        }
    }

    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        if !self.opening {
            return;
        }
        let length = self.offset.length().max(1e-3);
        self.progress = (self.progress + self.speed * ctx.dt() / length).min(1.0);
        self.place(ctx, me);
        if self.progress >= 1.0 {
            self.opening = false;
        }
    }

    fn interaction_prompt(&self) -> Option<String> {
        (self.progress == 0.0 && !self.opening).then(|| "Open door".to_owned())
    }

    fn on_interact(&mut self, ctx: &mut Ctx, _me: Entity, _by: Entity) {
        ctx.show_message("It won't budge. There must be a mechanism nearby.", 4.0);
    }

    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, _from: Option<Entity>) {
        if message == "open" && self.progress == 0.0 && !self.opening {
            self.opening = true;
            ctx.play_sound(me);
            ctx.state_mut().insert(DOOR_OPEN.into(), true.into());
            ctx.log(GameEvent::Custom {
                name: "door_opened".into(),
                detail: ctx.name(me),
            });
        }
    }
}

struct Lever {
    target: Option<psx_core::EntityId>,
    prompt: String,
    pulled: bool,
}

impl Behavior for Lever {
    fn start(&mut self, ctx: &mut Ctx, _me: Entity) {
        if ctx.state().get(DOOR_OPEN) == Some(&true.into()) {
            self.pulled = true;
        }
    }

    fn interaction_prompt(&self) -> Option<String> {
        (!self.pulled).then(|| self.prompt.clone())
    }

    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, _by: Entity) {
        self.pulled = true;
        if let Some(mut t) = ctx.local(me) {
            t.rotation = Quat::from_rotation_x(-0.6) * t.rotation;
            ctx.set_local(me, t);
        }
        ctx.play_sound(me);
        ctx.log(GameEvent::Custom {
            name: "lever_pulled".into(),
            detail: ctx.name(me),
        });
        match self.target.and_then(|id| ctx.find(id)) {
            Some(target) => ctx.send(target, Some(me), "open"),
            None => ctx.show_message("The lever moves, but nothing happens.", 3.0),
        }
    }
}

struct Inspect {
    prompt: String,
    text: String,
    seconds: f32,
}

impl Behavior for Inspect {
    fn interaction_prompt(&self) -> Option<String> {
        Some(self.prompt.clone())
    }

    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, _by: Entity) {
        ctx.show_message(self.text.clone(), self.seconds);
        ctx.log(GameEvent::Custom {
            name: "inspected".into(),
            detail: ctx.name(me),
        });
    }
}

struct Exit {
    message: String,
}

impl Behavior for Exit {
    fn on_trigger(&mut self, ctx: &mut Ctx, _me: Entity, other: Entity, entered: bool) {
        if entered && ctx.world().get::<&PlayerTag>(other).is_ok() {
            ctx.finish(self.message.clone());
        }
    }
}

struct Caretaker {
    prompt: String,
    text: String,
    idle: String,
    talk: String,
    fade: f32,
    talking: bool,
}

impl Behavior for Caretaker {
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        if self.talking && !ctx.is_animation_playing(me) {
            self.talking = false;
            ctx.play_animation(me, &self.idle, self.fade, true);
        }
    }

    fn interaction_prompt(&self) -> Option<String> {
        (!self.talking).then(|| self.prompt.clone())
    }

    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, _by: Entity) {
        if !self.text.is_empty() {
            ctx.show_message(self.text.clone(), 5.0);
        }
        self.talking = ctx.play_animation(me, &self.talk, self.fade, false);
        ctx.log(GameEvent::Custom {
            name: "talked".into(),
            detail: ctx.name(me),
        });
    }
}
