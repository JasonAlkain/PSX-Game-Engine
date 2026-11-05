//! Gameplay script files, shared by the game (which runs them) and the
//! editor (which lists, creates and reports on them).
//!
//! Every `scripts/**/*.lua` file of a project is a behaviour kind named
//! [`LUA_KIND_PREFIX`] followed by its path without the extension, folders
//! joined by dots: `scripts/puzzles/door.lua` is `lua.puzzles.door`. The
//! scene format allows only `a-z`, `0-9`, `_` and `.` in kinds, at most
//! [`MAX_KIND_LEN`] characters, so script and folder names follow that
//! rule ([`check_lua_kind`]).
//!
//! A problem with a script (it does not load, or a callback failed while
//! the game ran) is a [`ScriptDiagnostic`]. The game lists them in its
//! behaviour schema, its headless reports and, during the editor's Play,
//! on stdout ([`crate::play_control::PlayMessage`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::project_settings::SCRIPTS_DIR;

/// Prefix of every Lua behaviour kind.
pub const LUA_KIND_PREFIX: &str = "lua.";
/// Longest behaviour kind the scene format accepts.
pub const MAX_KIND_LEN: usize = 64;
/// How deep folders under `scripts/` are searched.
pub const MAX_DEPTH: u32 = 8;

/// A problem with one script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptDiagnostic {
    /// Project-relative script path, `/`-separated.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Behaviour kind (`lua.door`).
    #[serde(default)]
    pub kind: String,
    /// Entity name, for runtime errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// Callback that failed (`fixed_update`), for runtime errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    pub message: String,
    /// Fixed tick of a runtime error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
}

impl ScriptDiagnostic {
    /// Whether the game found it while running (a callback failed), as
    /// opposed to while loading the script.
    pub fn is_runtime(&self) -> bool {
        self.callback.is_some() || self.tick.is_some()
    }

    /// `file:line`, or the file alone.
    pub fn location(&self) -> String {
        match self.line {
            Some(l) => format!("{}:{l}", self.file),
            None => self.file.clone(),
        }
    }
}

impl std::fmt::Display for ScriptDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.location(), self.message)?;
        match (&self.entity, &self.callback) {
            (Some(e), Some(c)) => write!(
                f,
                " (in {c} on \"{e}\"; behaviour {} switched off)",
                self.kind
            ),
            (Some(e), None) => write!(f, " (on \"{e}\", behaviour {})", self.kind),
            _ => Ok(()),
        }
    }
}

/// The behaviour kind of `scripts/<rel>` (`rel` is `/`-separated).
pub fn lua_kind_for(rel: &str) -> String {
    format!(
        "{LUA_KIND_PREFIX}{}",
        rel.strip_suffix(".lua").unwrap_or(rel).replace('/', ".")
    )
}

/// The script path (relative to `scripts/`) of a Lua kind, or `None` for
/// other kinds.
pub fn lua_script_for(kind: &str) -> Option<String> {
    let rest = kind.strip_prefix(LUA_KIND_PREFIX)?;
    (!rest.is_empty()).then(|| format!("{}.lua", rest.replace('.', "/")))
}

/// Kinds follow the scene format's rule; explains how to rename the file
/// when one does not.
pub fn check_lua_kind(kind: &str) -> Result<(), String> {
    let ok = kind.len() <= MAX_KIND_LEN
        && kind.split('.').all(|part| !part.is_empty())
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{kind}` is not a valid behaviour kind: name scripts and their folders with \
             a-z, 0-9 and _ only (no spaces, capitals, dots or dashes), at most {} characters \
             in all; not loaded",
            MAX_KIND_LEN - LUA_KIND_PREFIX.len()
        ))
    }
}

/// The `.lua` files under the project's `scripts/` folder, relative to it
/// with `/` separators, sorted. Dotfiles and dot-folders are skipped.
pub fn find_lua_scripts(project_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(&project_root.join(SCRIPTS_DIR), "", 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, prefix: &str, depth: u32, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            walk(&path, &rel, depth + 1, out);
        } else if name.ends_with(".lua") && name.len() > 4 {
            out.push(rel);
        }
    }
}

/// Checks a name typed for a new script (`door`, `puzzles/door`, with or
/// without `.lua`) and returns its path relative to `scripts/`.
pub fn new_script_path(name: &str) -> Result<String, String> {
    let name = name.trim().replace('\\', "/");
    let name = name.trim_matches('/');
    let name = name.strip_suffix(".lua").unwrap_or(name);
    if name.is_empty() {
        return Err("type a name for the script".into());
    }
    if name
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("folder names cannot be empty, `.` or `..`".into());
    }
    let rel = format!("{name}.lua");
    let kind = lua_kind_for(&rel);
    let ok = kind.len() <= MAX_KIND_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/');
    if !ok {
        return Err(format!(
            "use a-z, 0-9 and _ only (folders with /), at most {} characters",
            MAX_KIND_LEN - LUA_KIND_PREFIX.len()
        ));
    }
    Ok(rel)
}

/// The text of a new Lua script.
pub fn lua_template(kind: &str) -> String {
    format!(
        r#"-- {kind}: attach it to an entity in the inspector (Add component).
-- Fields appear in the inspector; their values are in `self`.
-- Guide: https://jasonalkain.github.io/PSX-Game-Engine/guide/lua.html
return {{
  description = "",
  fields = {{
    {{ name = "speed", default = 1, help = "an example field" }},
  }},

  -- Once, on the first tick after the entity appears.
  start = function(self, ctx)
  end,

  -- Every fixed tick (60 per second).
  fixed_update = function(self, ctx)
  end,
}}
"#
    )
}

/// Writes a new Lua script from [`lua_template`] at `scripts/<rel>`
/// (`rel` from [`new_script_path`]), making folders as needed. Refuses to
/// replace an existing file. Returns the file's path.
pub fn create_lua_script(project_root: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = project_root.join(SCRIPTS_DIR).join(rel);
    if path.exists() {
        return Err(format!("{SCRIPTS_DIR}/{rel} already exists"));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, lua_template(&lua_kind_for(rel)))
        .map_err(|e| format!("could not write {SCRIPTS_DIR}/{rel}: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_paths_map_both_ways() {
        assert_eq!(lua_kind_for("door.lua"), "lua.door");
        assert_eq!(lua_kind_for("puzzles/door.lua"), "lua.puzzles.door");
        assert_eq!(
            lua_script_for("lua.puzzles.door").as_deref(),
            Some("puzzles/door.lua")
        );
        assert_eq!(lua_script_for("psx.player"), None);
        assert_eq!(lua_script_for("lua."), None);
        assert!(check_lua_kind("lua.puzzles.door").is_ok());
        let e = check_lua_kind("lua.Door").unwrap_err();
        assert!(e.contains("with a-z, 0-9 and _ only"), "{e}");
        assert!(check_lua_kind("lua.a..b").is_err());
        assert!(check_lua_kind(&format!("lua.{}", "a".repeat(61))).is_err());
    }

    #[test]
    fn new_script_names_are_checked() {
        assert_eq!(new_script_path(" door ").unwrap(), "door.lua");
        assert_eq!(
            new_script_path("puzzles/door.lua").unwrap(),
            "puzzles/door.lua"
        );
        assert_eq!(
            new_script_path("puzzles\\door").unwrap(),
            "puzzles/door.lua"
        );
        for bad in [
            "",
            "Door",
            "my door",
            "a-b",
            "../x",
            "a//b",
            "a.b",
            &"x".repeat(61),
        ] {
            assert!(new_script_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn new_scripts_are_written_once_and_found() {
        let root = std::env::temp_dir().join(format!("cf_scripts_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = create_lua_script(&root, "puzzles/door.lua").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("-- lua.puzzles.door:"));
        assert!(create_lua_script(&root, "puzzles/door.lua").is_err());
        std::fs::write(root.join("scripts/a.lua"), "return {}").unwrap();
        std::fs::write(root.join("scripts/.hidden.lua"), "return {}").unwrap();
        std::fs::write(root.join("scripts/notes.txt"), "").unwrap();
        assert_eq!(find_lua_scripts(&root), ["a.lua", "puzzles/door.lua"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnostics_read_back_and_print() {
        let d = ScriptDiagnostic {
            file: "scripts/door.lua".into(),
            line: Some(7),
            kind: "lua.door".into(),
            entity: Some("Door".into()),
            callback: Some("on_message".into()),
            message: "boom".into(),
            tick: Some(12),
        };
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(serde_json::from_value::<ScriptDiagnostic>(json).unwrap(), d);
        assert!(d.is_runtime());
        assert_eq!(
            d.to_string(),
            "scripts/door.lua:7: boom (in on_message on \"Door\"; behaviour lua.door switched off)"
        );
        let load: ScriptDiagnostic =
            serde_json::from_str(r#"{"file":"scripts/x.lua","message":"no return"}"#).unwrap();
        assert!(!load.is_runtime());
        assert_eq!(load.location(), "scripts/x.lua");
    }
}
