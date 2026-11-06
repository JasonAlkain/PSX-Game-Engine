//! The gameplay API that scripts see as `ctx`, shared by every script
//! language (Lua and C#).
//!
//! A script calls a method by name with plain values ([`ScriptValue`]);
//! [`call`] checks the arguments, runs it on [`Ctx`] and returns plain
//! values. Each language only converts its own values to and from these,
//! so Lua and C# behave the same and a new method is added in one place.
//! Entities are integers ([`entity_value`]), vectors three numbers, angles
//! degrees in yaw, pitch, roll order.

use glam::{EulerRot, Quat, Vec3};
use serde_json::Value as Json;

use copperframe_assets::project::Project;

use crate::api::{Entity, GameEvent, Registry, ScriptDiagnostic};
use crate::sim::Ctx;

/// Loads the project's scripts of every language that is on in
/// `project.json` into `registry`. Returns the problems; those scripts
/// are not registered.
pub fn register_project_scripts(
    project: &Project,
    registry: &mut Registry,
) -> Vec<ScriptDiagnostic> {
    let mut problems = crate::lua::register_project_scripts(project, registry);
    problems.extend(crate::csharp::register_project_scripts(project, registry));
    problems
}

/// A value passed between a script and the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
}

impl ScriptValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            ScriptValue::Nil => "nil",
            ScriptValue::Bool(_) => "boolean",
            ScriptValue::Int(_) | ScriptValue::Num(_) => "number",
            ScriptValue::Str(_) => "string",
        }
    }
}

impl From<bool> for ScriptValue {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}
impl From<f32> for ScriptValue {
    fn from(x: f32) -> Self {
        Self::Num(f64::from(x))
    }
}
impl From<f64> for ScriptValue {
    fn from(x: f64) -> Self {
        Self::Num(x)
    }
}
impl From<u64> for ScriptValue {
    fn from(x: u64) -> Self {
        Self::Int(x as i64)
    }
}
impl From<String> for ScriptValue {
    fn from(s: String) -> Self {
        Self::Str(s)
    }
}
impl From<Entity> for ScriptValue {
    fn from(e: Entity) -> Self {
        Self::Int(entity_value(e))
    }
}
impl<T: Into<ScriptValue>> From<Option<T>> for ScriptValue {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Nil, Into::into)
    }
}

/// A script's view of an entity handle.
pub fn entity_value(e: Entity) -> i64 {
    e.to_bits().get() as i64
}

/// The entity of a script handle.
pub fn value_entity(v: i64) -> Result<Entity, String> {
    Entity::from_bits(v as u64).ok_or_else(|| format!("{v} is not an entity"))
}

/// `(yaw, pitch, roll)` in degrees.
pub fn degrees(q: Quat) -> (f32, f32, f32) {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    (y.to_degrees(), x.to_degrees(), z.to_degrees())
}

pub fn from_degrees(yaw: f32, pitch: f32, roll: f32) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        yaw.to_radians(),
        pitch.to_radians(),
        roll.to_radians(),
    )
}

/// A game state value as a script value (structures written by Rust code
/// become JSON text).
pub fn json_to_value(v: &Json) -> ScriptValue {
    match v {
        Json::Null => ScriptValue::Nil,
        Json::Bool(b) => ScriptValue::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => ScriptValue::Int(i),
            None => ScriptValue::Num(n.as_f64().unwrap_or(0.0)),
        },
        Json::String(s) => ScriptValue::Str(s.clone()),
        other => ScriptValue::Str(other.to_string()),
    }
}

fn value_to_json(v: &ScriptValue) -> Result<Option<Json>, String> {
    Ok(match v {
        ScriptValue::Nil => None,
        ScriptValue::Bool(b) => Some(Json::from(*b)),
        ScriptValue::Int(i) => Some(Json::from(*i)),
        ScriptValue::Num(x) if x.is_finite() => Some(Json::from(*x)),
        ScriptValue::Num(_) => return Err("game state cannot hold NaN or infinity".into()),
        ScriptValue::Str(s) => Some(Json::from(s.clone())),
    })
}

/// The arguments of one call, with typed, 1-based access and errors that
/// name the argument.
struct Args<'a> {
    values: &'a [ScriptValue],
}

impl Args<'_> {
    fn get(&self, i: usize) -> &ScriptValue {
        self.values.get(i - 1).unwrap_or(&ScriptValue::Nil)
    }

    fn wrong(&self, i: usize, want: &str) -> String {
        format!(
            "argument {i}: expected {want}, found {}",
            self.get(i).type_name()
        )
    }

    fn opt_num(&self, i: usize) -> Result<Option<f32>, String> {
        match self.get(i) {
            ScriptValue::Nil => Ok(None),
            ScriptValue::Int(n) => Ok(Some(*n as f32)),
            ScriptValue::Num(x) => Ok(Some(*x as f32)),
            ScriptValue::Str(s) => s
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| self.wrong(i, "a number")),
            ScriptValue::Bool(_) => Err(self.wrong(i, "a number")),
        }
    }

    fn num(&self, i: usize) -> Result<f32, String> {
        self.opt_num(i)?.ok_or_else(|| self.wrong(i, "a number"))
    }

    fn vec(&self, first: usize) -> Result<Vec3, String> {
        Ok(Vec3::new(
            self.num(first)?,
            self.num(first + 1)?,
            self.num(first + 2)?,
        ))
    }

    fn opt_bool(&self, i: usize) -> Result<Option<bool>, String> {
        match self.get(i) {
            ScriptValue::Nil => Ok(None),
            ScriptValue::Bool(b) => Ok(Some(*b)),
            _ => Err(self.wrong(i, "a boolean")),
        }
    }

    fn bool(&self, i: usize) -> Result<bool, String> {
        self.opt_bool(i)?.ok_or_else(|| self.wrong(i, "a boolean"))
    }

    fn opt_str(&self, i: usize) -> Result<Option<String>, String> {
        match self.get(i) {
            ScriptValue::Nil => Ok(None),
            ScriptValue::Str(s) => Ok(Some(s.clone())),
            ScriptValue::Int(n) => Ok(Some(n.to_string())),
            ScriptValue::Num(x) => Ok(Some(x.to_string())),
            ScriptValue::Bool(_) => Err(self.wrong(i, "a string")),
        }
    }

    fn str(&self, i: usize) -> Result<String, String> {
        self.opt_str(i)?.ok_or_else(|| self.wrong(i, "a string"))
    }

    fn opt_entity(&self, i: usize) -> Result<Option<Entity>, String> {
        match self.get(i) {
            ScriptValue::Nil => Ok(None),
            ScriptValue::Int(n) => value_entity(*n).map(Some),
            ScriptValue::Num(x) if x.fract() == 0.0 => value_entity(*x as i64).map(Some),
            _ => Err(self.wrong(i, "an entity")),
        }
    }

    fn entity(&self, i: usize) -> Result<Entity, String> {
        self.opt_entity(i)?
            .ok_or_else(|| self.wrong(i, "an entity"))
    }
}

fn missing(e: Entity) -> String {
    format!("entity {} has no transform (despawned?)", entity_value(e))
}

fn none() -> Vec<ScriptValue> {
    Vec::new()
}

fn one(v: impl Into<ScriptValue>) -> Vec<ScriptValue> {
    vec![v.into()]
}

fn three(v: Vec3) -> Vec<ScriptValue> {
    vec![v.x.into(), v.y.into(), v.z.into()]
}

/// Runs the `ctx` method `name` for a script of entity `me`. Errors are
/// `name: what went wrong`; the language adds its own prefix and the
/// script's line.
pub fn call(
    ctx: &mut Ctx,
    me: Entity,
    name: &str,
    values: &[ScriptValue],
) -> Result<Vec<ScriptValue>, String> {
    run(ctx, me, name, &Args { values }).map_err(|e| format!("{name}: {e}"))
}

fn run(ctx: &mut Ctx, me: Entity, name: &str, a: &Args) -> Result<Vec<ScriptValue>, String> {
    let local = |ctx: &Ctx, e: Entity| ctx.local(e).ok_or_else(|| missing(e));
    Ok(match name {
        // Time and identity.
        "dt" => one(ctx.dt()),
        "tick" => one(ctx.tick()),
        "me" => one(me),
        "find" => {
            let id = a.str(1)?;
            one(id.parse().ok().and_then(|id| ctx.find(id)))
        }
        "find_by_name" => one(ctx.find_by_name(&a.str(1)?)),
        "id_of" => one(ctx.id_of(a.entity(1)?).map(|id| id.to_string())),
        "name" => one(ctx.name(a.entity(1)?)),
        "exists" => one(a.entity(1).is_ok_and(|e| ctx.exists(e))),
        "parent" => one(ctx.parent(a.entity(1)?)),

        // Transforms (local to the parent).
        "position" => three(local(ctx, a.entity(1)?)?.translation),
        "set_position" | "translate" => {
            let e = a.entity(1)?;
            let v = a.vec(2)?;
            let mut t = local(ctx, e)?;
            t.translation = if name == "translate" {
                t.translation + v
            } else {
                v
            };
            ctx.set_local(e, t);
            none()
        }
        "world_position" => {
            let e = a.entity(1)?;
            three(ctx.world_position(e).ok_or_else(|| missing(e))?)
        }
        "rotation" => {
            let (y, p, r) = degrees(local(ctx, a.entity(1)?)?.rotation);
            vec![y.into(), p.into(), r.into()]
        }
        "set_rotation" => {
            let e = a.entity(1)?;
            let mut t = local(ctx, e)?;
            t.rotation = from_degrees(a.num(2)?, a.num(3)?, a.num(4)?);
            ctx.set_local(e, t);
            none()
        }
        "rotate" => {
            let e = a.entity(1)?;
            let (y, p, r) = (
                a.num(2)?,
                a.opt_num(3)?.unwrap_or(0.0),
                a.opt_num(4)?.unwrap_or(0.0),
            );
            let mut t = local(ctx, e)?;
            t.rotation = (t.rotation * from_degrees(y, p, r)).normalize();
            ctx.set_local(e, t);
            none()
        }
        "scale" => three(local(ctx, a.entity(1)?)?.scale),
        "set_scale" => {
            let e = a.entity(1)?;
            let mut t = local(ctx, e)?;
            t.scale = a.vec(2)?;
            ctx.set_local(e, t);
            none()
        }
        "set_hidden" => {
            ctx.set_hidden(a.entity(1)?, a.bool(2)?);
            none()
        }

        // Physics.
        "velocity" => three(ctx.velocity(a.entity(1)?)),
        "set_velocity" | "apply_impulse" | "add_force" => {
            let (e, v) = (a.entity(1)?, a.vec(2)?);
            match name {
                "set_velocity" => ctx.set_velocity(e, v),
                "apply_impulse" => ctx.apply_impulse(e, v),
                _ => ctx.add_force(e, v),
            }
            none()
        }
        "raycast" => {
            let (origin, dir, max) = (a.vec(1)?, a.vec(4)?, a.num(7)?);
            let ignore = a.opt_entity(8)?;
            match ctx.raycast(origin, dir, max, ignore) {
                Some(h) => vec![h.entity.into(), h.distance.into()],
                None => vec![ScriptValue::Nil, ScriptValue::Nil],
            }
        }

        // Sound and animation.
        "play_sound" => {
            ctx.play_sound(a.entity(1)?);
            none()
        }
        "stop_sound" => {
            ctx.stop_sound(a.entity(1)?, a.opt_num(2)?.unwrap_or(0.0));
            none()
        }
        "is_sound_playing" => one(ctx.is_sound_playing(a.entity(1)?)),
        "play_animation" => {
            let (e, clip) = (a.entity(1)?, a.str(2)?);
            let fade = a.opt_num(3)?.unwrap_or(0.2);
            let looping = a.opt_bool(4)?.unwrap_or(true);
            one(ctx.play_animation(e, &clip, fade, looping))
        }
        "stop_animation" => {
            ctx.stop_animation(a.entity(1)?, a.opt_num(2)?.unwrap_or(0.2));
            none()
        }
        "is_animation_playing" => one(ctx.is_animation_playing(a.entity(1)?)),

        // Input.
        "held" => one(ctx.input().held(&a.str(1)?)),
        "pressed" => one(ctx.input().pressed(&a.str(1)?)),
        "axis" => one(ctx.input().axis(&a.str(1)?, &a.str(2)?)),

        // Messages, events, state and flow.
        "send" => {
            let (to, message) = (a.entity(1)?, a.str(2)?);
            ctx.send(to, Some(me), message);
            none()
        }
        "show_message" => {
            let text = a.str(1)?;
            ctx.show_message(text, a.opt_num(2)?.unwrap_or(3.0));
            none()
        }
        "event" => {
            let name = a.str(1)?;
            let detail = a.opt_str(2)?.unwrap_or_default();
            ctx.log(GameEvent::Custom { name, detail });
            none()
        }
        "get_state" => {
            let key = a.str(1)?;
            one(ctx
                .state()
                .get(&key)
                .map_or(ScriptValue::Nil, json_to_value))
        }
        "set_state" => {
            let key = a.str(1)?;
            match value_to_json(a.get(2))? {
                Some(v) => ctx.state_mut().insert(key, v),
                None => ctx.state_mut().remove(&key),
            };
            none()
        }
        "despawn" => {
            ctx.despawn(a.entity(1)?);
            none()
        }
        "load_scene" => {
            ctx.load_scene(a.str(1)?);
            none()
        }
        "finish" => {
            ctx.finish(a.opt_str(1)?.unwrap_or_default());
            none()
        }
        "log" => {
            let text = a.str(1)?;
            tracing::info!(target: "script", entity = %ctx.name(me), "{text}");
            none()
        }
        _ => return Err("no such method (see the scripting pages of the wiki)".into()),
    })
}

/// Every `ctx` method, for documentation tests and the C# bindings.
pub const METHODS: &[&str] = &[
    "dt",
    "tick",
    "me",
    "find",
    "find_by_name",
    "id_of",
    "name",
    "exists",
    "parent",
    "position",
    "set_position",
    "translate",
    "world_position",
    "rotation",
    "set_rotation",
    "rotate",
    "scale",
    "set_scale",
    "set_hidden",
    "velocity",
    "set_velocity",
    "apply_impulse",
    "add_force",
    "raycast",
    "play_sound",
    "stop_sound",
    "is_sound_playing",
    "play_animation",
    "stop_animation",
    "is_animation_playing",
    "held",
    "pressed",
    "axis",
    "send",
    "show_message",
    "event",
    "get_state",
    "set_state",
    "despawn",
    "load_scene",
    "finish",
    "log",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_handles_round_trip_through_integers() {
        let mut world = hecs::World::new();
        let a = world.spawn(());
        world.despawn(a).unwrap();
        let b = world.spawn(()); // same index, newer generation
        for e in [a, b] {
            assert_eq!(value_entity(entity_value(e)).unwrap(), e);
        }
        assert!(value_entity(0).is_err());
    }

    #[test]
    fn angles_are_degrees_yaw_pitch_roll() {
        let q = from_degrees(90.0, 10.0, -5.0);
        let (y, p, r) = degrees(q);
        assert!((y - 90.0).abs() < 1e-3 && (p - 10.0).abs() < 1e-3 && (r + 5.0).abs() < 1e-3);
    }

    #[test]
    fn arguments_are_checked_by_position() {
        let v = [ScriptValue::Int(3), ScriptValue::Str("x".into())];
        let a = Args { values: &v };
        assert_eq!(a.num(1).unwrap(), 3.0);
        assert!(
            a.num(2)
                .unwrap_err()
                .contains("argument 2: expected a number, found string")
        );
        assert_eq!(a.opt_num(3).unwrap(), None);
        assert!(a.bool(3).unwrap_err().contains("found nil"));
        assert_eq!(a.str(1).unwrap(), "3");
    }
}
