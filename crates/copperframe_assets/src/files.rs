//! Plain project files and folders, as the editor's Assets panel manages
//! them: making folders and small text or data files, moving things to the
//! project's bin (`.psx/deleted/`), and finding which scenes mention
//! something before it goes.
//!
//! Paths given and returned are relative to the project root with `/`
//! separators. Only the three folders the panel shows can be changed:
//! [`ASSETS_DIR`], [`SCENES_DIR`] and [`SCRIPTS_DIR`], never the folders
//! themselves.

use std::path::{Path, PathBuf};

use crate::meta::{AssetKind, META_SUFFIX};
use crate::project::{ASSETS_DIR, SCENES_DIR};
use crate::scripts::SCRIPTS_DIR;

/// Where deleted files go, relative to the project. They stay there until
/// the user empties it by hand, so a delete can be undone in a file
/// manager.
pub const DELETED_DIR: &str = ".psx/deleted";

/// The project folders the editor manages.
pub const ROOTS: [&str; 3] = [ASSETS_DIR, SCENES_DIR, SCRIPTS_DIR];

/// A small file the editor can create in `assets/`. They ship with an
/// exported game like every other file there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFile {
    /// Plain text (`.txt`): notes, dialogue, credits.
    Text,
    /// JSON data (`.json`): tables of numbers or strings.
    Json,
}

impl DataFile {
    pub const ALL: [DataFile; 2] = [DataFile::Text, DataFile::Json];

    pub fn extension(self) -> &'static str {
        match self {
            DataFile::Text => "txt",
            DataFile::Json => "json",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DataFile::Text => "Text file",
            DataFile::Json => "JSON data file",
        }
    }

    /// What a new file holds.
    pub fn template(self) -> &'static str {
        match self {
            DataFile::Text => "",
            DataFile::Json => "{\n}\n",
        }
    }
}

/// The root folder `rel` is in (`assets`, `scenes` or `scripts`), if any.
pub fn root_of(rel: &str) -> Option<&'static str> {
    ROOTS
        .into_iter()
        .find(|root| rel == *root || rel.starts_with(&format!("{root}/")))
}

/// Checks a project-relative path: inside one of [`ROOTS`], with no empty,
/// `.` or `..` parts and no hidden ones.
fn check_inside(rel: &str) -> Result<(), String> {
    if root_of(rel).is_none() {
        return Err(format!(
            "only files in {ASSETS_DIR}/, {SCENES_DIR}/ and {SCRIPTS_DIR}/ can be changed here"
        ));
    }
    if rel
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == ".." || p.starts_with('.'))
    {
        return Err(format!("{rel} is not a plain project path"));
    }
    Ok(())
}

/// Checks a name typed for a new file or folder: one plain name that works
/// on every platform. Returns it trimmed.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("type a name".into());
    }
    if name.starts_with('.') {
        return Err("names cannot start with a dot".into());
    }
    if name.ends_with('.') {
        return Err("names cannot end with a dot".into());
    }
    if let Some(c) = name.chars().find(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
    }) {
        return Err(format!("names cannot contain `{c}`"));
    }
    // Windows reserves these, whatever the extension.
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("`{name}` is a reserved name on Windows"));
    }
    Ok(name.to_owned())
}

/// Makes the folder `name` in `parent` (relative to the project, inside
/// one of [`ROOTS`]). Returns its path.
pub fn create_folder(project_root: &Path, parent: &str, name: &str) -> Result<PathBuf, String> {
    let parent = parent.trim_matches('/');
    check_inside(parent)?;
    let name = check_name(name)?;
    let path = project_root.join(parent).join(&name);
    if path.exists() {
        return Err(format!("{parent}/{name} already exists"));
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("could not make {parent}/{name}: {e}"))?;
    Ok(path)
}

/// The file name a new data file called `name` gets: the extension is
/// added unless typed.
pub fn data_file_name(name: &str, kind: DataFile) -> Result<String, String> {
    let name = check_name(name)?;
    let ext = format!(".{}", kind.extension());
    if name.to_ascii_lowercase().ends_with(&ext) {
        if name.len() == ext.len() {
            return Err("type a name".into());
        }
        Ok(name)
    } else {
        Ok(format!("{name}{ext}"))
    }
}

/// Writes a new data file in `folder` (under `assets/`) from its template.
/// Refuses to replace a file. Returns its path.
pub fn create_data_file(
    project_root: &Path,
    folder: &str,
    name: &str,
    kind: DataFile,
) -> Result<PathBuf, String> {
    let folder = folder.trim_matches('/');
    check_inside(folder)?;
    if root_of(folder) != Some(ASSETS_DIR) {
        return Err(format!(
            "text and data files go in {ASSETS_DIR}/, which is copied into exported games"
        ));
    }
    let file = data_file_name(name, kind)?;
    let path = project_root.join(folder).join(&file);
    if path.exists() {
        return Err(format!("{folder}/{file} already exists"));
    }
    std::fs::create_dir_all(project_root.join(folder))
        .map_err(|e| format!("could not make {folder}: {e}"))?;
    std::fs::write(&path, kind.template())
        .map_err(|e| format!("could not write {folder}/{file}: {e}"))?;
    Ok(path)
}

/// Moves a file or folder (relative to the project) into
/// [`DELETED_DIR`]. A file's asset sidecar (`.meta.json`) and backup
/// (`.bak`) go with it. A name already in the bin gets a number in front.
/// Returns where it went, relative to the project.
pub fn move_to_bin(project_root: &Path, rel: &str) -> Result<String, String> {
    let rel = rel.trim_matches('/');
    check_inside(rel)?;
    if ROOTS.contains(&rel) {
        return Err(format!("{rel}/ is one of the project's own folders"));
    }
    let path = project_root.join(rel);
    if !path.exists() {
        return Err(format!("{rel} does not exist"));
    }
    let bin = project_root.join(DELETED_DIR);
    std::fs::create_dir_all(&bin).map_err(|e| format!("could not make {DELETED_DIR}: {e}"))?;
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let mut target = bin.join(name);
    let mut n = 1;
    while target.exists() || sidecar(&target).exists() {
        target = bin.join(format!("{n}-{name}"));
        n += 1;
    }
    std::fs::rename(&path, &target).map_err(|e| format!("could not delete {rel}: {e}"))?;
    if target.is_file() {
        if sidecar(&path).is_file() {
            let _ = std::fs::rename(sidecar(&path), sidecar(&target));
        }
        let _ = std::fs::remove_file(format!("{}.bak", path.display()));
    }
    Ok(format!(
        "{DELETED_DIR}/{}",
        target.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn sidecar(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}{META_SUFFIX}", path.display()))
}

/// What the panel shows for a file that is not a registered asset or a
/// scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `scripts/**/*.lua`.
    Lua,
    /// `scripts/**/*.cs`.
    CSharp,
    /// Anything else: text, data, a model's buffers.
    Other,
}

impl FileKind {
    pub fn of(rel: &str) -> FileKind {
        let lower = rel.to_ascii_lowercase();
        if root_of(rel) == Some(SCRIPTS_DIR) && lower.ends_with(".lua") {
            FileKind::Lua
        } else if root_of(rel) == Some(SCRIPTS_DIR) && lower.ends_with(".cs") {
            FileKind::CSharp
        } else {
            FileKind::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileKind::Lua => "Lua script",
            FileKind::CSharp => "C# script",
            FileKind::Other => "file",
        }
    }

    pub fn is_script(self) -> bool {
        self != FileKind::Other
    }
}

/// How deep [`plain_files`] and [`folders`] look.
const MAX_DEPTH: u32 = 12;

/// Files the panel lists besides assets and scenes: everything under
/// `scripts/`, and the files under `assets/` that are neither importable
/// sources nor sidecars. Hidden files, backups and temporary files are
/// left out. Sorted.
pub fn plain_files(project_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for root in [ASSETS_DIR, SCRIPTS_DIR] {
        walk(&project_root.join(root), root, 0, &mut |rel, is_dir| {
            if is_dir {
                return;
            }
            let skip = rel.ends_with(".bak")
                || rel.ends_with(".tmp")
                || (root == ASSETS_DIR
                    && (rel.ends_with(META_SUFFIX)
                        || AssetKind::for_path(Path::new(rel)).is_some()));
            if !skip {
                out.push(rel.to_owned());
            }
        });
    }
    out.sort();
    out
}

/// Every folder under [`ROOTS`] (and the roots themselves), including
/// empty ones; hidden folders are left out. Sorted, so a folder comes right
/// before its children.
pub fn folders(project_root: &Path) -> Vec<String> {
    let mut out: Vec<String> = ROOTS.iter().map(|r| (*r).to_owned()).collect();
    for root in ROOTS {
        walk(&project_root.join(root), root, 0, &mut |rel, is_dir| {
            if is_dir {
                out.push(rel.to_owned());
            }
        });
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, prefix: &str, depth: u32, f: &mut dyn FnMut(&str, bool)) {
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
        let rel = format!("{prefix}/{name}");
        let Ok(ty) = entry.file_type() else {
            continue;
        };
        if ty.is_dir() {
            f(&rel, true);
            walk(&entry.path(), &rel, depth + 1, f);
        } else if ty.is_file() {
            f(&rel, false);
        }
    }
}

/// The scenes (project-relative) whose file mentions `needle` as a JSON
/// string (an asset ID, a behaviour kind, a file path), with how many
/// times. Unreadable scenes are skipped.
pub fn scenes_mentioning(project_root: &Path, needle: &str) -> Vec<(String, usize)> {
    let quoted = format!("\"{needle}\"");
    let mut out = Vec::new();
    walk(
        &project_root.join(SCENES_DIR),
        SCENES_DIR,
        0,
        &mut |rel, is_dir| {
            if is_dir || !rel.ends_with(".scene.json") {
                return;
            }
            if let Ok(text) = std::fs::read_to_string(project_root.join(rel)) {
                let n = text.matches(&quoted).count();
                if n > 0 {
                    out.push((rel.to_owned(), n));
                }
            }
        },
    );
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cf_files_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  props ").unwrap(), "props");
        assert_eq!(
            check_name("Level 2 notes.txt").unwrap(),
            "Level 2 notes.txt"
        );
        for bad in [
            "", " ", ".hidden", "a/b", "a\\b", "what?", "dots.", "CON", "com1.txt", "x:y",
        ] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(check_name("console").is_ok());
        assert_eq!(
            data_file_name("credits", DataFile::Text).unwrap(),
            "credits.txt"
        );
        assert_eq!(
            data_file_name("loot.JSON", DataFile::Json).unwrap(),
            "loot.JSON"
        );
        assert!(data_file_name(".json", DataFile::Json).is_err());
    }

    #[test]
    fn roots_and_paths_outside_them_are_refused() {
        assert_eq!(root_of("assets/props/a.glb"), Some(ASSETS_DIR));
        assert_eq!(root_of("scripts"), Some(SCRIPTS_DIR));
        assert_eq!(root_of("assetsx/a"), None);
        let root = temp("refuse");
        for rel in [
            "assets",
            "scenes",
            "game/src",
            "project.json",
            "assets/../game",
            ".psx/x",
        ] {
            assert!(move_to_bin(&root, rel).is_err(), "{rel}");
        }
        assert!(create_folder(&root, "game", "x").is_err());
        assert!(create_data_file(&root, "scripts", "notes", DataFile::Text).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_list_and_delete() {
        let root = temp("cycle");
        let props = create_folder(&root, "assets", "props").unwrap();
        assert!(props.is_dir());
        assert!(create_folder(&root, "assets", "props").is_err(), "exists");
        create_folder(&root, "scripts", "puzzles").unwrap();
        let notes = create_data_file(&root, "assets/props", "notes", DataFile::Text).unwrap();
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "");
        let loot = create_data_file(&root, "assets", "loot", DataFile::Json).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&loot).unwrap())
                .is_ok()
        );
        assert!(create_data_file(&root, "assets", "loot", DataFile::Json).is_err());
        std::fs::write(root.join("assets/props/crate.png"), b"x").unwrap();
        std::fs::write(root.join("assets/props/crate.png.meta.json"), b"{}").unwrap();
        std::fs::write(root.join("assets/props/crate.png.bak"), b"x").unwrap();
        std::fs::write(root.join("scripts/puzzles/door.lua"), b"return {}").unwrap();
        std::fs::write(root.join("scripts/Door.cs"), b"").unwrap();
        std::fs::write(root.join("scripts/.hidden.lua"), b"").unwrap();
        assert_eq!(
            plain_files(&root),
            [
                "assets/loot.json",
                "assets/props/notes.txt",
                "scripts/Door.cs",
                "scripts/puzzles/door.lua"
            ]
        );
        assert_eq!(FileKind::of("scripts/Door.cs"), FileKind::CSharp);
        assert_eq!(FileKind::of("scripts/puzzles/door.lua"), FileKind::Lua);
        assert_eq!(FileKind::of("assets/door.lua"), FileKind::Other);
        assert_eq!(
            folders(&root),
            [
                "assets",
                "assets/props",
                "scenes",
                "scripts",
                "scripts/puzzles"
            ]
        );

        // An asset goes with its sidecar; its backup is dropped.
        let gone = move_to_bin(&root, "assets/props/crate.png").unwrap();
        assert_eq!(gone, ".psx/deleted/crate.png");
        assert!(root.join(".psx/deleted/crate.png.meta.json").is_file());
        assert!(!root.join("assets/props/crate.png.meta.json").exists());
        assert!(!root.join("assets/props/crate.png.bak").exists());
        // A second file of the same name does not replace the first.
        std::fs::write(root.join("assets/crate.png"), b"y").unwrap();
        assert_eq!(
            move_to_bin(&root, "assets/crate.png").unwrap(),
            ".psx/deleted/1-crate.png"
        );
        // Folders go whole.
        assert_eq!(
            move_to_bin(&root, "scripts/puzzles").unwrap(),
            ".psx/deleted/puzzles"
        );
        assert!(root.join(".psx/deleted/puzzles/door.lua").is_file());
        assert!(
            move_to_bin(&root, "scripts/puzzles").is_err(),
            "already gone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scenes_that_mention_something_are_found() {
        let root = temp("mention");
        std::fs::create_dir_all(root.join("scenes/levels")).unwrap();
        std::fs::write(
            root.join("scenes/main.scene.json"),
            r#"{"a":{"kind":"lua.door"},"b":{"kind":"lua.door"},"c":"lua.doorway"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/levels/b.scene.json"),
            r#"{"kind":"lua.door"}"#,
        )
        .unwrap();
        std::fs::write(root.join("scenes/notes.txt"), r#""lua.door""#).unwrap();
        assert_eq!(
            scenes_mentioning(&root, "lua.door"),
            [
                ("scenes/levels/b.scene.json".to_owned(), 1),
                ("scenes/main.scene.json".to_owned(), 2)
            ]
        );
        assert!(scenes_mentioning(&root, "lua.lever").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
