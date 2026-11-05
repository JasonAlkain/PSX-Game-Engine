//! Lua gameplay scripts.
//!
//! Every `scripts/**/*.lua` file of a project (when `scripting.lua` is on
//! in `project.json`) becomes a behaviour kind named `lua.` followed by
//! its path without the extension, folders joined by dots: for example
//! `scripts/puzzles/door.lua` is `lua.puzzles.door`. Behaviour kinds may
//! only use `a-z`, `0-9`, `_` and `.`, so script and folder names do too
//! (a script with another name is reported and not loaded). A script
//! returns a table:
//!
//! ```lua
//! return {
//!   description = "Spins while the player stands on it",
//!   fields = {
//!     { name = "speed", default = 90, help = "degrees per second" },
//!     { name = "target", kind = "entity", help = "what to spin" },
//!   },
//!   start = function(self, ctx) end,
//!   fixed_update = function(self, ctx) end,
//!   on_trigger = function(self, ctx, other, entered) end,
//!   on_collision = function(self, ctx, other, started) end,
//!   on_interact = function(self, ctx, by) end,
//!   on_message = function(self, ctx, message, from) end,
//!   on_destroy = function(self, ctx) end,
//! }
//! ```
//!
//! Scripts register in the same [`Registry`] as Rust behaviours, so
//! scenes, the editor's inspector, saves and headless runs treat them the
//! same way. `self` is a table per entity holding the field values and
//! `self.entity`; it falls back to the script table, so helper functions
//! there work as `self:helper()`. Setting `self.prompt` to a string makes
//! the entity interactive. `ctx` exposes a subset of [`Ctx`] (see
//! [`API_METHODS`]): entities are integers, vectors are three numbers, angles are
//! degrees.
//!
//! The Lua state is sandboxed: only the `table`, `string`, `math`, `utf8`
//! and `coroutine` libraries are loaded (no `io`, `os`, `require`,
//! `debug`, `dofile` or `loadfile`), `print` goes to the log, and `math.random` starts from a fixed
//! seed so headless and windowed runs match. A callback that raises an
//! error, or runs more than [`INSTRUCTION_BUDGET`] instructions, is
//! reported as a [`ScriptDiagnostic`] and that behaviour is switched off;
//! the rest of the game keeps running.

use std::cell::Cell;
use std::rc::Rc;

use copperframe_assets::project::Project;
use copperframe_assets::scripts::{SCRIPTS_DIR, check_lua_kind, find_lua_scripts};
use glam::{EulerRot, Quat, Vec3};
use mlua::{
    FromLuaMulti, Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, MultiValue, StdLib, Table,
    Value, Variadic, VmState,
};
use serde_json::Value as Json;

use crate::api::{
    Behavior, DynamicKind, Entity, FieldKind, GameEvent, Params, Registry, ScriptDiagnostic,
};
use crate::sim::Ctx;

pub use copperframe_assets::scripts::{LUA_KIND_PREFIX as KIND_PREFIX, lua_kind_for as kind_for};
/// Lua instructions one callback (or loading one script) may run before it
/// is stopped as a probable endless loop.
pub const INSTRUCTION_BUDGET: u32 = 10_000_000;
const HOOK_EVERY: u32 = 10_000;
/// Scripts larger than this are refused (a script is source code).
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

/// The Lua state shared by every script of one simulation.
struct Host {
    lua: Lua,
    budget: Rc<Cell<u32>>,
    /// The `ctx` table passed to callbacks (see [`CTX_PRELUDE`]).
    ctx: Table,
}

impl Host {
    fn new() -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
            LuaOptions::default(),
        )?;
        let budget = Rc::new(Cell::new(INSTRUCTION_BUDGET));
        let left = budget.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_EVERY),
            move |_, _| {
                let n = left.get();
                if n <= HOOK_EVERY {
                    return Err(mlua::Error::runtime(format!(
                        "stopped after {INSTRUCTION_BUDGET} instructions without returning \
                         (an endless loop?)"
                    )));
                }
                left.set(n - HOOK_EVERY);
                Ok(VmState::Continue)
            },
        )?;
        let print = lua.create_function(|_, args: Variadic<Value>| {
            let words: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| format!("{v:?}")))
                .collect();
            tracing::info!(target: "lua", "{}", words.join("\t"));
            Ok(())
        })?;
        lua.globals().set("print", print)?;
        // The base library can read files; scripts cannot.
        for name in ["dofile", "loadfile"] {
            lua.globals().set(name, Value::Nil)?;
        }
        lua.load("math.randomseed(0)").exec()?;
        let ctx: Table = lua.load(CTX_PRELUDE).set_name("=ctx").eval()?;
        Ok(Self { lua, budget, ctx })
    }
}

#[derive(Default)]
struct Callbacks {
    start: Option<Function>,
    fixed_update: Option<Function>,
    on_trigger: Option<Function>,
    on_collision: Option<Function>,
    on_interact: Option<Function>,
    on_message: Option<Function>,
    on_destroy: Option<Function>,
}

struct Script {
    kind: String,
    file: String,
    module: Table,
    callbacks: Callbacks,
    fields: Vec<(String, FieldKind)>,
}

/// A script's declared kind, before registration.
struct Loaded {
    script: Rc<Script>,
    description: String,
    fields: Vec<(String, FieldKind, Json, String)>,
}

/// Loads the project's Lua scripts into `registry`. Returns the problems
/// (a script that does not compile or declares bad fields); those scripts
/// are not registered.
pub fn register_project_scripts(
    project: &Project,
    registry: &mut Registry,
) -> Vec<ScriptDiagnostic> {
    if !project.file.scripting.lua {
        return Vec::new();
    }
    let files = find_lua_scripts(&project.root);
    if files.is_empty() {
        return Vec::new();
    }
    let host = match Host::new() {
        Ok(h) => Rc::new(h),
        Err(e) => {
            return vec![ScriptDiagnostic {
                file: SCRIPTS_DIR.into(),
                line: None,
                kind: String::new(),
                entity: None,
                callback: None,
                message: format!("could not start Lua: {e}"),
                tick: None,
            }];
        }
    };
    let mut problems = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for rel in files {
        let file = format!("{SCRIPTS_DIR}/{rel}");
        let kind = kind_for(&rel);
        let problem = |line: Option<u32>, message: String| ScriptDiagnostic {
            file: file.clone(),
            line,
            kind: kind.clone(),
            entity: None,
            callback: None,
            message,
            tick: None,
        };
        if let Err(m) = check_lua_kind(&kind) {
            problems.push(problem(None, m));
            continue;
        }
        if seen.contains(&kind) {
            problems.push(problem(
                None,
                format!("another script already has the kind `{kind}`; rename one of them"),
            ));
            continue;
        }
        seen.push(kind.clone());
        let path = project.root.join(&file);
        let source = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_SCRIPT_BYTES => {
                problems.push(problem(None, "larger than 1 MiB; not loaded".into()));
                continue;
            }
            _ => match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    problems.push(problem(None, format!("could not read: {e}")));
                    continue;
                }
            },
        };
        match load_script(&host, &file, &kind, &source) {
            Ok(loaded) => {
                let folder = match rel.rsplit_once('/') {
                    Some((dir, _)) => format!("Scripts/{dir}"),
                    None => "Scripts".into(),
                };
                let script = loaded.script.clone();
                let host = host.clone();
                registry.register_dynamic(DynamicKind {
                    kind: kind.clone(),
                    description: loaded.description,
                    fields: loaded.fields,
                    folder,
                    create: Rc::new(move |p| LuaBehavior::create(&host, &script, p)),
                });
            }
            Err(e) => {
                let (line, message) = describe(&e, &file);
                problems.push(problem(line, message));
            }
        }
    }
    problems
}

fn load_script(host: &Host, file: &str, kind: &str, source: &str) -> mlua::Result<Loaded> {
    host.budget.set(INSTRUCTION_BUDGET);
    let value: Value = host.lua.load(source).set_name(format!("@{file}")).eval()?;
    let Value::Table(module) = value else {
        return Err(mlua::Error::runtime(format!(
            "{file}: the script must return a table (`return {{ ... }}`), found {}",
            value.type_name()
        )));
    };
    let bad = |m: String| mlua::Error::runtime(format!("{file}: {m}"));
    let description = match module.get::<Value>("description")? {
        Value::Nil => String::new(),
        Value::String(s) => s.to_str()?.to_owned(),
        other => {
            return Err(bad(format!(
                "`description` must be a string, found {}",
                other.type_name()
            )));
        }
    };
    let mut callbacks = Callbacks::default();
    for (name, slot) in [
        ("start", &mut callbacks.start),
        ("fixed_update", &mut callbacks.fixed_update),
        ("on_trigger", &mut callbacks.on_trigger),
        ("on_collision", &mut callbacks.on_collision),
        ("on_interact", &mut callbacks.on_interact),
        ("on_message", &mut callbacks.on_message),
        ("on_destroy", &mut callbacks.on_destroy),
    ] {
        match module.get::<Value>(name)? {
            Value::Nil => {}
            Value::Function(f) => *slot = Some(f),
            other => {
                return Err(bad(format!(
                    "`{name}` must be a function, found {}",
                    other.type_name()
                )));
            }
        }
    }
    let mut fields = Vec::new();
    match module.get::<Value>("fields")? {
        Value::Nil => {}
        Value::Table(list) => {
            for (i, item) in list.sequence_values::<Value>().enumerate() {
                let Value::Table(f) = item? else {
                    return Err(bad(format!("fields[{}] must be a table", i + 1)));
                };
                fields.push(field(&f, i + 1).map_err(&bad)?);
            }
        }
        other => {
            return Err(bad(format!(
                "`fields` must be a list, found {}",
                other.type_name()
            )));
        }
    }
    for (i, (name, ..)) in fields.iter().enumerate() {
        if fields[..i].iter().any(|(n, ..)| n == name) {
            return Err(bad(format!("field `{name}` is declared twice")));
        }
    }
    let script = Rc::new(Script {
        kind: kind.to_owned(),
        file: file.to_owned(),
        module,
        callbacks,
        fields: fields.iter().map(|(n, k, ..)| (n.clone(), *k)).collect(),
    });
    Ok(Loaded {
        script,
        description,
        fields,
    })
}

/// One `fields` entry: `{name, default?, kind?, help?}`.
fn field(f: &Table, n: usize) -> Result<(String, FieldKind, Json, String), String> {
    let get = |key: &str| f.get::<Value>(key).map_err(|e| e.to_string());
    let name = match get("name")? {
        Value::String(s) => s.to_string_lossy(),
        _ => return Err(format!("fields[{n}] needs a `name` string")),
    };
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ident || name == "entity" || name == "prompt" {
        return Err(format!(
            "field name `{name}` must be a Lua identifier other than `entity` and `prompt`"
        ));
    }
    let help = match get("help")? {
        Value::Nil => String::new(),
        Value::String(s) => s.to_string_lossy(),
        _ => return Err(format!("field `{name}`: `help` must be a string")),
    };
    let declared = match get("kind")? {
        Value::Nil => None,
        Value::String(s) => Some(s.to_string_lossy()),
        _ => return Err(format!("field `{name}`: `kind` must be a string")),
    };
    let default = get("default")?;
    let (kind, json) = match (declared.as_deref(), default) {
        (Some("entity"), Value::Nil) => (FieldKind::Entity, Json::from("")),
        (Some("entity"), _) => {
            return Err(format!("field `{name}`: an entity field has no default"));
        }
        (None | Some("bool"), Value::Boolean(b)) => (FieldKind::Bool, Json::from(b)),
        (None | Some("number"), Value::Integer(i)) => (FieldKind::Number, Json::from(i as f64)),
        (None | Some("number"), Value::Number(x)) if x.is_finite() => {
            (FieldKind::Number, Json::from(x))
        }
        (None | Some("text"), Value::String(s)) => {
            (FieldKind::Text, Json::from(s.to_string_lossy()))
        }
        (Some("bool"), Value::Nil) => (FieldKind::Bool, Json::from(false)),
        (Some("number"), Value::Nil) => (FieldKind::Number, Json::from(0.0)),
        (Some("text"), Value::Nil) => (FieldKind::Text, Json::from("")),
        (Some(k @ ("bool" | "number" | "text")), v) => {
            return Err(format!(
                "field `{name}`: a {k} field cannot default to a {}",
                v.type_name()
            ));
        }
        (Some(k), _) => {
            return Err(format!(
                "field `{name}`: unknown kind `{k}` (bool, number, text or entity)"
            ));
        }
        (None, v) => {
            return Err(format!(
                "field `{name}`: give a boolean, number or string `default`, or `kind = \"entity\"` \
                 (found {})",
                v.type_name()
            ));
        }
    };
    Ok((name, kind, json, help))
}

/// Line and message of a Lua error, without the file prefix and the
/// traceback.
fn describe(e: &mlua::Error, file: &str) -> (Option<u32>, String) {
    let (text, traceback) = match e {
        mlua::Error::CallbackError { traceback, cause } => (cause.to_string(), traceback.clone()),
        other => (other.to_string(), String::new()),
    };
    let first = text.lines().next().unwrap_or_default();
    let first = first
        .trim_start_matches("runtime error: ")
        .trim_start_matches("syntax error: ");
    let at = format!("{file}:");
    let line_of = |s: &str| -> Option<(u32, usize)> {
        let i = s.find(&at)?;
        let rest = &s[i + at.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let n = digits.parse().ok()?;
        rest[digits.len()..]
            .starts_with(':')
            .then_some((n, i + at.len() + digits.len() + 1))
    };
    if let Some((n, end)) = line_of(first) {
        return (Some(n), first[end..].trim().to_owned());
    }
    // A Rust callback failed (bad argument and the like); the script's line
    // is in the traceback.
    let line = line_of(&traceback).map(|(n, _)| n);
    let message = first
        .strip_prefix(&format!("{file}: "))
        .unwrap_or(first)
        .trim()
        .to_owned();
    (line, message)
}

/// A behaviour backed by a Lua script.
struct LuaBehavior {
    host: Rc<Host>,
    script: Rc<Script>,
    /// `self` of the script's callbacks.
    instance: Table,
    entity_set: bool,
    /// Switched off after an error.
    off: bool,
}

impl LuaBehavior {
    fn create(
        host: &Rc<Host>,
        script: &Rc<Script>,
        p: &Params,
    ) -> Result<Box<dyn Behavior>, String> {
        let make = || -> mlua::Result<Table> {
            let lua = &host.lua;
            let instance = lua.create_table()?;
            for (name, kind) in &script.fields {
                let v = match kind {
                    FieldKind::Bool => Value::Boolean(p.bool(name)),
                    FieldKind::Number => Value::Number(f64::from(p.number(name))),
                    FieldKind::Text | FieldKind::Entity => {
                        Value::String(lua.create_string(p.text(name))?)
                    }
                };
                instance.raw_set(name.as_str(), v)?;
            }
            let meta = lua.create_table()?;
            meta.raw_set("__index", script.module.clone())?;
            instance.set_metatable(Some(meta))?;
            Ok(instance)
        };
        let instance = make().map_err(|e| e.to_string())?;
        Ok(Box::new(Self {
            host: host.clone(),
            script: script.clone(),
            instance,
            entity_set: false,
            off: false,
        }))
    }

    fn call(
        &mut self,
        ctx: &mut Ctx,
        me: Entity,
        name: &str,
        pick: fn(&Callbacks) -> &Option<Function>,
        args: impl IntoLuaMulti,
    ) {
        if self.off {
            return;
        }
        let Some(f) = pick(&self.script.callbacks).clone() else {
            return;
        };
        if !self.entity_set {
            self.entity_set = self.instance.raw_set("entity", to_lua(me)).is_ok();
        }
        self.host.budget.set(INSTRUCTION_BUDGET);
        let instance = self.instance.clone();
        let api = self.host.ctx.clone();
        let result = self.host.lua.scope(|scope| {
            let d = scope.create_function_mut(
                |lua, (name, values): (mlua::LuaString, MultiValue)| {
                    dispatch(lua, &mut *ctx, me, &name.to_str()?, values)
                },
            )?;
            api.raw_set("_d", d)?;
            let r = f.call::<()>((instance, api.clone(), args));
            api.raw_set("_d", Value::Nil)?;
            r
        });
        if let Err(e) = result {
            self.off = true;
            let (line, message) = describe(&e, &self.script.file);
            let entity = ctx.name(me);
            ctx.report_script_problem(ScriptDiagnostic {
                file: self.script.file.clone(),
                line,
                kind: self.script.kind.clone(),
                entity: Some(entity),
                callback: Some(name.to_owned()),
                message,
                tick: None,
            });
        }
    }
}

impl Behavior for LuaBehavior {
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, "start", |c| &c.start, ());
    }
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, "fixed_update", |c| &c.fixed_update, ());
    }
    fn on_trigger(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, entered: bool) {
        self.call(
            ctx,
            me,
            "on_trigger",
            |c| &c.on_trigger,
            (to_lua(other), entered),
        );
    }
    fn on_collision(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, started: bool) {
        self.call(
            ctx,
            me,
            "on_collision",
            |c| &c.on_collision,
            (to_lua(other), started),
        );
    }
    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, by: Entity) {
        self.call(ctx, me, "on_interact", |c| &c.on_interact, to_lua(by));
    }
    fn interaction_prompt(&self) -> Option<String> {
        if self.off {
            return None;
        }
        match self.instance.raw_get::<Value>("prompt") {
            Ok(Value::String(s)) => Some(s.to_string_lossy()),
            _ => None,
        }
    }
    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, from: Option<Entity>) {
        let msg = message.to_owned();
        self.call(
            ctx,
            me,
            "on_message",
            |c| &c.on_message,
            (msg, from.map(to_lua)),
        );
    }
    fn on_destroy(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, "on_destroy", |c| &c.on_destroy, ());
    }
}

/// Lua's view of an entity handle.
fn to_lua(e: Entity) -> i64 {
    e.to_bits().get() as i64
}

fn entity(v: i64) -> mlua::Result<Entity> {
    Entity::from_bits(v as u64).ok_or_else(|| mlua::Error::runtime(format!("{v} is not an entity")))
}

fn missing(e: Entity) -> mlua::Error {
    mlua::Error::runtime(format!(
        "entity {} has no transform (despawned?)",
        to_lua(e)
    ))
}

fn degrees(q: Quat) -> (f32, f32, f32) {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    (y.to_degrees(), x.to_degrees(), z.to_degrees())
}

fn from_degrees(yaw: f32, pitch: f32, roll: f32) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        yaw.to_radians(),
        pitch.to_radians(),
        roll.to_radians(),
    )
}

fn json_to_lua(lua: &Lua, v: &Json) -> mlua::Result<Value> {
    Ok(match v {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(0.0)),
        },
        Json::String(s) => Value::String(lua.create_string(s)?),
        // Saves written by Rust code may hold structures; scripts see them
        // as JSON text.
        other => Value::String(lua.create_string(other.to_string())?),
    })
}

fn lua_to_json(v: Value) -> mlua::Result<Option<Json>> {
    Ok(match v {
        Value::Nil => None,
        Value::Boolean(b) => Some(Json::from(b)),
        Value::Integer(i) => Some(Json::from(i)),
        Value::Number(x) if x.is_finite() => Some(Json::from(x)),
        Value::String(s) => Some(Json::from(s.to_str()?.to_owned())),
        other => {
            return Err(mlua::Error::runtime(format!(
                "game state holds nil, booleans, numbers and strings, not a {}",
                other.type_name()
            )));
        }
    })
}

/// Lua side of `ctx`: one table reused for every call. Its methods
/// forward to the per-call dispatch function stored in `_d`, so a call
/// creates one Rust closure instead of a userdata with a fresh metatable
/// of every method (about 40x cheaper). A `ctx` kept past its callback has
/// no `_d` and raises an error.
const CTX_PRELUDE: &str = r#"
local methods = setmetatable({}, { __index = function(cache, name)
  local f = function(self, ...)
    local d = rawget(self, "_d")
    if d == nil then
      error("ctx can only be used during the callback it was passed to", 2)
    end
    return d(name, ...)
  end
  cache[name] = f
  return f
end })
return setmetatable({}, { __index = function(_, name) return methods[name] end })
"#;

fn args<A: FromLuaMulti>(lua: &Lua, name: &str, values: MultiValue) -> mlua::Result<A> {
    A::from_lua_multi(values, lua).map_err(|e| mlua::Error::runtime(format!("ctx:{name}: {e}")))
}

fn ret(lua: &Lua, v: impl IntoLuaMulti) -> mlua::Result<MultiValue> {
    v.into_lua_multi(lua)
}

/// Runs `ctx:<name>(...)` for a script of entity `me`. The methods are a
/// subset of [`Ctx`]: entities are integers, vectors three numbers,
/// angles degrees (yaw, pitch, roll).
fn dispatch(
    lua: &Lua,
    ctx: &mut Ctx,
    me: Entity,
    name: &str,
    values: MultiValue,
) -> mlua::Result<MultiValue> {
    macro_rules! a {
        ($t:ty) => {
            args::<$t>(lua, name, values)?
        };
    }
    let local = |ctx: &Ctx, e: Entity| ctx.local(e).ok_or_else(|| missing(e));
    match name {
        // Time and identity.
        "dt" => ret(lua, ctx.dt()),
        "tick" => ret(lua, ctx.tick()),
        "me" => ret(lua, to_lua(me)),
        "find" => {
            let id = a!(String);
            ret(lua, id.parse().ok().and_then(|id| ctx.find(id)).map(to_lua))
        }
        "find_by_name" => {
            let n = a!(String);
            ret(lua, ctx.find_by_name(&n).map(to_lua))
        }
        "id_of" => {
            let e = entity(a!(i64))?;
            ret(lua, ctx.id_of(e).map(|id| id.to_string()))
        }
        "name" => {
            let e = entity(a!(i64))?;
            ret(lua, ctx.name(e))
        }
        "exists" => {
            let e = a!(i64);
            ret(lua, entity(e).is_ok_and(|e| ctx.exists(e)))
        }
        "parent" => {
            let e = entity(a!(i64))?;
            ret(lua, ctx.parent(e).map(to_lua))
        }

        // Transforms (local to the parent).
        "position" => {
            let t = local(ctx, entity(a!(i64))?)?.translation;
            ret(lua, (t.x, t.y, t.z))
        }
        "set_position" | "translate" => {
            let (e, x, y, z) = a!((i64, f32, f32, f32));
            let e = entity(e)?;
            let mut t = local(ctx, e)?;
            let v = Vec3::new(x, y, z);
            t.translation = if name == "translate" {
                t.translation + v
            } else {
                v
            };
            ctx.set_local(e, t);
            ret(lua, ())
        }
        "world_position" => {
            let e = entity(a!(i64))?;
            let p = ctx.world_position(e).ok_or_else(|| missing(e))?;
            ret(lua, (p.x, p.y, p.z))
        }
        "rotation" => ret(lua, degrees(local(ctx, entity(a!(i64))?)?.rotation)),
        "set_rotation" => {
            let (e, y, p, r) = a!((i64, f32, f32, f32));
            let e = entity(e)?;
            let mut t = local(ctx, e)?;
            t.rotation = from_degrees(y, p, r);
            ctx.set_local(e, t);
            ret(lua, ())
        }
        "rotate" => {
            let (e, y, p, r) = a!((i64, f32, Option<f32>, Option<f32>));
            let e = entity(e)?;
            let mut t = local(ctx, e)?;
            t.rotation =
                (t.rotation * from_degrees(y, p.unwrap_or(0.0), r.unwrap_or(0.0))).normalize();
            ctx.set_local(e, t);
            ret(lua, ())
        }
        "scale" => {
            let s = local(ctx, entity(a!(i64))?)?.scale;
            ret(lua, (s.x, s.y, s.z))
        }
        "set_scale" => {
            let (e, x, y, z) = a!((i64, f32, f32, f32));
            let e = entity(e)?;
            let mut t = local(ctx, e)?;
            t.scale = Vec3::new(x, y, z);
            ctx.set_local(e, t);
            ret(lua, ())
        }
        "set_hidden" => {
            let (e, hidden) = a!((i64, bool));
            ctx.set_hidden(entity(e)?, hidden);
            ret(lua, ())
        }

        // Physics.
        "velocity" => {
            let v = ctx.velocity(entity(a!(i64))?);
            ret(lua, (v.x, v.y, v.z))
        }
        "set_velocity" | "apply_impulse" | "add_force" => {
            let (e, x, y, z) = a!((i64, f32, f32, f32));
            let (e, v) = (entity(e)?, Vec3::new(x, y, z));
            match name {
                "set_velocity" => ctx.set_velocity(e, v),
                "apply_impulse" => ctx.apply_impulse(e, v),
                _ => ctx.add_force(e, v),
            }
            ret(lua, ())
        }
        "raycast" => {
            let (ox, oy, oz, dx, dy, dz, max, ignore) =
                a!((f32, f32, f32, f32, f32, f32, f32, Option<i64>));
            let ignore = ignore.map(entity).transpose()?;
            let hit = ctx.raycast(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz), max, ignore);
            ret(
                lua,
                hit.map_or((None, None), |h| (Some(to_lua(h.entity)), Some(h.distance))),
            )
        }

        // Sound and animation.
        "play_sound" => {
            ctx.play_sound(entity(a!(i64))?);
            ret(lua, ())
        }
        "stop_sound" => {
            let (e, fade) = a!((i64, Option<f32>));
            ctx.stop_sound(entity(e)?, fade.unwrap_or(0.0));
            ret(lua, ())
        }
        "is_sound_playing" => {
            let e = entity(a!(i64))?;
            ret(lua, ctx.is_sound_playing(e))
        }
        "play_animation" => {
            let (e, clip, fade, looping) = a!((i64, String, Option<f32>, Option<bool>));
            let played = ctx.play_animation(
                entity(e)?,
                &clip,
                fade.unwrap_or(0.2),
                looping.unwrap_or(true),
            );
            ret(lua, played)
        }
        "stop_animation" => {
            let (e, fade) = a!((i64, Option<f32>));
            ctx.stop_animation(entity(e)?, fade.unwrap_or(0.2));
            ret(lua, ())
        }
        "is_animation_playing" => {
            let e = entity(a!(i64))?;
            ret(lua, ctx.is_animation_playing(e))
        }

        // Input.
        "held" => {
            let action = a!(String);
            ret(lua, ctx.input().held(&action))
        }
        "pressed" => {
            let action = a!(String);
            ret(lua, ctx.input().pressed(&action))
        }
        "axis" => {
            let (neg, pos) = a!((String, String));
            ret(lua, ctx.input().axis(&neg, &pos))
        }

        // Messages, events, state and flow.
        "send" => {
            let (to, message) = a!((i64, String));
            ctx.send(entity(to)?, Some(me), message);
            ret(lua, ())
        }
        "show_message" => {
            let (text, secs) = a!((String, Option<f32>));
            ctx.show_message(text, secs.unwrap_or(3.0));
            ret(lua, ())
        }
        "event" => {
            let (name, detail) = a!((String, Option<String>));
            ctx.log(GameEvent::Custom {
                name,
                detail: detail.unwrap_or_default(),
            });
            ret(lua, ())
        }
        "get_state" => {
            let key = a!(String);
            let v = ctx
                .state()
                .get(&key)
                .map_or(Ok(Value::Nil), |v| json_to_lua(lua, v))?;
            ret(lua, v)
        }
        "set_state" => {
            let (key, value) = a!((String, Value));
            match lua_to_json(value)? {
                Some(v) => ctx.state_mut().insert(key, v),
                None => ctx.state_mut().remove(&key),
            };
            ret(lua, ())
        }
        "despawn" => {
            ctx.despawn(entity(a!(i64))?);
            ret(lua, ())
        }
        "load_scene" => {
            ctx.load_scene(a!(String));
            ret(lua, ())
        }
        "finish" => {
            ctx.finish(a!(Option<String>).unwrap_or_default());
            ret(lua, ())
        }
        "log" => {
            let text = a!(String);
            tracing::info!(target: "lua", entity = %ctx.name(me), "{text}");
            ret(lua, ())
        }
        _ => Err(mlua::Error::runtime(format!(
            "ctx has no method `{name}` (see the Lua scripting page of the wiki)"
        ))),
    }
}

/// Every `ctx` method, for documentation tests.
pub const API_METHODS: &[&str] = &[
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
mod tests;
