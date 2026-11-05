//! C# gameplay scripts.
//!
//! When `scripting.csharp` is on in `project.json`, the project's
//! `scripts/**/*.cs` are compiled into one assembly
//! ([`copperframe_assets::csharp::ensure_built`]) and loaded into the .NET
//! runtime, hosted in the game process ([`ffi`]). Each public class
//! deriving from `Copperframe.Behaviour` becomes a behaviour kind
//! `cs.<class name in snake_case>` in the same [`Registry`] as Rust and Lua
//! behaviours. Its callbacks get a `Ctx` whose methods go through the
//! shared [`script_api`], so C# and Lua scripts have the same API.
//!
//! A callback that throws is reported as a [`ScriptDiagnostic`] with the
//! script's file and line (from the assembly's portable PDB) and that
//! behaviour is switched off; the rest of the game keeps running. Unlike
//! Lua, C# is not sandboxed (it is trusted code, like Rust game code) and
//! an endless loop is not stopped.

mod ffi;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use copperframe_assets::csharp::{Build, CS_KIND_PREFIX, ensure_built};
use copperframe_assets::project::Project;
use copperframe_assets::scripts::{MAX_KIND_LEN, SCRIPTS_DIR};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::api::{Behavior, DynamicKind, Entity, FieldKind, Params, Registry, ScriptDiagnostic};
use crate::script_api::{self, entity_value};
use crate::sim::Ctx;

pub use ffi::ABI_VERSION;

thread_local! {
    /// One load per assembly per thread: .NET cannot unload an assembly
    /// from the default context, and Restart reloads the scene, not the
    /// scripts.
    static LOADED: RefCell<HashMap<PathBuf, Rc<ffi::Runtime>>> = RefCell::new(HashMap::new());
}

#[derive(Deserialize)]
struct Described {
    kinds: Vec<DescribedKind>,
    #[serde(default)]
    problems: Vec<DescribedProblem>,
}

#[derive(Deserialize)]
struct DescribedKind {
    kind: String,
    class: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    folder: String,
    fields: Vec<DescribedField>,
}

#[derive(Deserialize)]
struct DescribedField {
    name: String,
    kind: String,
    #[serde(default)]
    default: Json,
    #[serde(default)]
    help: String,
}

#[derive(Deserialize)]
struct DescribedProblem {
    class: Option<String>,
    message: String,
}

#[derive(Deserialize)]
struct ThrownError {
    message: String,
    file: Option<String>,
    #[serde(default)]
    line: u32,
}

fn problem(kind: &str, message: impl Into<String>) -> ScriptDiagnostic {
    ScriptDiagnostic {
        file: SCRIPTS_DIR.into(),
        line: None,
        kind: kind.into(),
        entity: None,
        callback: None,
        message: message.into(),
        tick: None,
    }
}

fn check_kind(kind: &str, class: &str) -> Result<(), String> {
    let name = &kind[CS_KIND_PREFIX.len().min(kind.len())..];
    let ok = kind.len() <= MAX_KIND_LEN
        && !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{kind}` (class {class}) is not a valid behaviour kind: use a-z, 0-9 and _ in \
             [Behaviour(Name = ...)], at most {} characters; not loaded",
            MAX_KIND_LEN - CS_KIND_PREFIX.len()
        ))
    }
}

/// Builds and loads the project's C# scripts into `registry`. Returns the
/// problems (compiler errors, a missing .NET, a class that cannot be
/// created); the affected kinds are not registered.
pub fn register_project_scripts(
    project: &Project,
    registry: &mut Registry,
) -> Vec<ScriptDiagnostic> {
    if !project.file.scripting.csharp {
        return Vec::new();
    }
    let dll = match ensure_built(&project.root, &mut |line| tracing::info!("{line}")) {
        Build::Nothing => return Vec::new(),
        Build::Failed(problems) => return problems,
        Build::Ready(dll) => dll,
    };
    let runtime = match load(&dll, &project.root) {
        Ok(r) => r,
        Err(e) => return vec![problem("", e)],
    };
    let described: Described = match serde_json::from_str(&runtime.describe()) {
        Ok(d) => d,
        Err(e) => {
            return vec![problem(
                "",
                format!("the scripts assembly answered oddly: {e}"),
            )];
        }
    };
    let mut problems: Vec<ScriptDiagnostic> = described
        .problems
        .into_iter()
        .map(|p| {
            let class = p.class.unwrap_or_default();
            problem(&class, p.message)
        })
        .collect();
    let mut seen: Vec<String> = Vec::new();
    for (index, k) in described.kinds.into_iter().enumerate() {
        if let Err(e) = check_kind(&k.kind, &k.class) {
            problems.push(problem(&k.kind, e));
            continue;
        }
        if seen.contains(&k.kind) || registry.get(&k.kind).is_some() {
            problems.push(problem(
                &k.kind,
                format!(
                    "class {} has the behaviour kind `{}`, which another behaviour already \
                     has; rename it or set [Behaviour(Name = ...)]",
                    k.class, k.kind
                ),
            ));
            continue;
        }
        seen.push(k.kind.clone());
        let mut fields = Vec::new();
        for f in k.fields {
            let kind = match f.kind.as_str() {
                "number" => FieldKind::Number,
                "bool" => FieldKind::Bool,
                "text" => FieldKind::Text,
                _ => FieldKind::Entity,
            };
            fields.push((f.name, kind, f.default, f.help));
        }
        let names: Rc<[(String, FieldKind)]> =
            fields.iter().map(|(n, k, ..)| (n.clone(), *k)).collect();
        let folder = match k.folder.trim_matches('/') {
            "" => "Scripts".to_owned(),
            f => format!("Scripts/{f}"),
        };
        let rt = runtime.clone();
        let kind = k.kind.clone();
        registry.register_dynamic(DynamicKind {
            kind: k.kind,
            description: k.description,
            fields,
            folder,
            create: Rc::new(move |p| CsBehavior::create(&rt, index, &kind, &names, p)),
        });
    }
    problems
}

fn load(dll: &Path, root: &Path) -> Result<Rc<ffi::Runtime>, String> {
    if let Some(rt) = LOADED.with(|l| l.borrow().get(dll).cloned()) {
        return Ok(rt);
    }
    let rt = Rc::new(ffi::Runtime::load(dll, root)?);
    LOADED.with(|l| l.borrow_mut().insert(dll.to_path_buf(), rt.clone()));
    Ok(rt)
}

/// A behaviour backed by a C# class.
struct CsBehavior {
    runtime: Rc<ffi::Runtime>,
    handle: i64,
    kind: String,
    /// Switched off after an error.
    off: bool,
}

impl CsBehavior {
    fn create(
        runtime: &Rc<ffi::Runtime>,
        index: usize,
        kind: &str,
        fields: &[(String, FieldKind)],
        p: &Params,
    ) -> Result<Box<dyn Behavior>, String> {
        let mut values = serde_json::Map::new();
        for (name, k) in fields {
            let v = match k {
                FieldKind::Bool => Json::from(p.bool(name)),
                FieldKind::Number => Json::from(f64::from(p.number(name))),
                FieldKind::Text | FieldKind::Entity => Json::from(p.text(name)),
            };
            values.insert(name.clone(), v);
        }
        let handle = runtime.create(index, &Json::Object(values).to_string())?;
        Ok(Box::new(Self {
            runtime: runtime.clone(),
            handle,
            kind: kind.to_owned(),
            off: false,
        }))
    }

    fn call(
        &mut self,
        ctx: &mut Ctx,
        me: Entity,
        callback: ffi::Callback,
        other: Option<Entity>,
        flag: bool,
        text: &str,
    ) {
        if self.off {
            return;
        }
        let other = other.map_or(0, entity_value);
        let result = {
            let mut dispatch = |method: &str, args: &[script_api::ScriptValue]| {
                script_api::call(&mut *ctx, me, method, args)
            };
            self.runtime.invoke(
                self.handle,
                callback,
                entity_value(me),
                other,
                flag,
                text,
                &mut dispatch,
            )
        };
        if let Err(json) = result {
            self.off = true;
            let thrown: ThrownError = serde_json::from_str(&json).unwrap_or(ThrownError {
                message: json,
                file: None,
                line: 0,
            });
            let entity = ctx.name(me);
            ctx.report_script_problem(ScriptDiagnostic {
                file: thrown.file.unwrap_or_else(|| SCRIPTS_DIR.into()),
                line: (thrown.line > 0).then_some(thrown.line),
                kind: self.kind.clone(),
                entity: Some(entity),
                callback: Some(callback.name().to_owned()),
                message: thrown.message,
                tick: None,
            });
        }
    }
}

impl Drop for CsBehavior {
    fn drop(&mut self) {
        self.runtime.release(self.handle);
    }
}

impl Behavior for CsBehavior {
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, ffi::Callback::Start, None, false, "");
    }
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, ffi::Callback::FixedUpdate, None, false, "");
    }
    fn on_trigger(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, entered: bool) {
        self.call(ctx, me, ffi::Callback::OnTrigger, Some(other), entered, "");
    }
    fn on_collision(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, started: bool) {
        self.call(
            ctx,
            me,
            ffi::Callback::OnCollision,
            Some(other),
            started,
            "",
        );
    }
    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, by: Entity) {
        self.call(ctx, me, ffi::Callback::OnInteract, Some(by), false, "");
    }
    fn interaction_prompt(&self) -> Option<String> {
        if self.off {
            return None;
        }
        self.runtime.prompt(self.handle)
    }
    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, from: Option<Entity>) {
        self.call(ctx, me, ffi::Callback::OnMessage, from, false, message);
    }
    fn on_destroy(&mut self, ctx: &mut Ctx, me: Entity) {
        self.call(ctx, me, ffi::Callback::OnDestroy, None, false, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_scene_rule() {
        assert!(check_kind("cs.door_opener", "DoorOpener").is_ok());
        let e = check_kind("cs.Door", "Door").unwrap_err();
        assert!(
            e.contains("class Door") && e.contains("a-z, 0-9 and _"),
            "{e}"
        );
        assert!(check_kind("cs.", "X").is_err());
        assert!(check_kind("cs.a.b", "X").is_err());
    }
}
