//! Building a project's C# scripts.
//!
//! Every `scripts/**/*.cs` file of a project (when `scripting.csharp` is on
//! in `project.json`) is compiled with the engine's C# API
//! ([`API_SOURCES`]) into one assembly by `dotnet build`, in
//! `.psx/csharp/`. Each public class deriving from `Copperframe.Behaviour`
//! is a behaviour kind `cs.<class name in snake_case>`; the game
//! (`copperframe_game::csharp`) hosts the .NET runtime and loads the
//! assembly.
//!
//! A build is skipped while the sources are unchanged ([`stamp`]). Each
//! successful build is copied to `.psx/csharp/out/<stamp>/` and
//! `.psx/csharp/current.txt` names it, so a running game keeps its copy
//! while the editor builds a new one (Windows locks loaded assemblies).
//! An export carries only that copy; a project with no `.cs` files but a
//! built copy (an export) loads it without building.
//!
//! Building needs the .NET SDK ([`DOTNET_VERSION`] or newer); running needs
//! only the .NET runtime.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::scripts::{MAX_DEPTH, MAX_KIND_LEN, SCRIPTS_DIR, ScriptDiagnostic};

/// Prefix of every C# behaviour kind.
pub const CS_KIND_PREFIX: &str = "cs.";
/// Folder (relative to the project) of the generated project and builds.
pub const BUILD_DIR: &str = ".psx/csharp";
/// Name of the compiled assembly.
pub const ASSEMBLY: &str = "CopperframeScripts";
/// The .NET version scripts target (newer runtimes run it too).
pub const DOTNET_VERSION: &str = "10.0";
/// Where to get .NET.
pub const DOTNET_URL: &str = "https://dotnet.microsoft.com/download";

/// The engine's C# sources compiled into every project's assembly, as
/// `(file name, text)`. Their entry points match `copperframe_game::csharp`.
pub const API_SOURCES: &[(&str, &str)] = &[
    ("Copperframe.cs", include_str!("../csharp/Copperframe.cs")),
    ("Host.cs", include_str!("../csharp/Host.cs")),
];

const PROJECT_FILE: &str = "CopperframeScripts.csproj";
const CURRENT_FILE: &str = "current.txt";
/// How long a build waits for another one (the editor's and the game's)
/// to finish.
const LOCK_WAIT: Duration = Duration::from_secs(120);

/// The `.cs` files under `scripts/`, relative to it, sorted.
pub fn find_cs_scripts(project_root: &Path) -> Vec<String> {
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
        // bin/ and obj/ appear when someone opens scripts/ in an IDE.
        if name.starts_with('.') || name == "bin" || name == "obj" {
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
        } else if name.ends_with(".cs") && name.len() > 3 {
            out.push(rel);
        }
    }
}

/// snake_case of a C# name, as the scripts host makes behaviour kinds:
/// `DoorOpener` is `door_opener`, `HTTPServer` is `http_server`.
pub fn snake_case(name: &str) -> String {
    let c: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &ch) in c.iter().enumerate() {
        if ch.is_uppercase() {
            let boundary = i > 0
                && (c[i - 1].is_lowercase()
                    || c[i - 1].is_ascii_digit()
                    || (i + 1 < c.len() && c[i + 1].is_lowercase() && c[i - 1].is_uppercase()));
            if boundary {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The behaviour kind of `scripts/<rel>` when its class is named after the
/// file (the convention; `[Behaviour(Name = ...)]` can change it).
pub fn cs_kind_for(rel: &str) -> String {
    let stem = rel.rsplit('/').next().unwrap_or(rel);
    let stem = stem.strip_suffix(".cs").unwrap_or(stem);
    format!("{CS_KIND_PREFIX}{}", snake_case(stem))
}

/// The `scripts/`-relative file of a C# behaviour kind, found by the
/// file-name convention.
pub fn cs_script_for(project_root: &Path, kind: &str) -> Option<String> {
    kind.strip_prefix(CS_KIND_PREFIX)?;
    find_cs_scripts(project_root)
        .into_iter()
        .find(|rel| cs_kind_for(rel) == kind)
}

/// Checks a name typed in the New script dialog (`DoorOpener`,
/// `Puzzles/DoorOpener`) and returns the file it becomes, relative to
/// `scripts/` (`Puzzles/DoorOpener.cs`).
pub fn new_cs_script_path(name: &str) -> Result<String, String> {
    let name = name.trim().replace('\\', "/");
    let name = name.trim_matches('/');
    let name = name.strip_suffix(".cs").unwrap_or(name);
    if name.is_empty() {
        return Err("type a class name for the script".into());
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err("folder names cannot be empty, `.` or `..`".into());
    }
    let ident = |p: &str| {
        p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !p.starts_with(|c: char| c.is_ascii_digit())
    };
    if !parts.iter().all(|p| ident(p)) {
        return Err("use A-Z, a-z, 0-9 and _ (folders with /); do not start with a digit".into());
    }
    let class = parts[parts.len() - 1];
    if !class.starts_with(|c: char| c.is_ascii_uppercase()) {
        return Err("start the class name with a capital letter, like DoorOpener".into());
    }
    let rel = format!("{name}.cs");
    if cs_kind_for(&rel).len() > MAX_KIND_LEN {
        return Err(format!(
            "the class name is too long (its kind has at most {} characters)",
            MAX_KIND_LEN - CS_KIND_PREFIX.len()
        ));
    }
    Ok(rel)
}

/// The text of a new C# script for `scripts/<rel>`. Subfolders become the
/// Add component folder.
pub fn cs_template(rel: &str) -> String {
    let (folder, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    let class = file.strip_suffix(".cs").unwrap_or(file);
    let kind = cs_kind_for(rel);
    let attr = if folder.is_empty() {
        "[Behaviour(\"\")]".to_owned()
    } else {
        format!("[Behaviour(\"\", Folder = \"{folder}\")]")
    };
    format!(
        r#"// {kind}: attach it to an entity in the inspector (Add component).
// Public fields appear in the inspector.
// Guide: https://jasonalkain.github.io/PSX-Game-Engine/guide/csharp.html
using Copperframe;

{attr}
public class {class} : Behaviour
{{
    [Help("an example field")] public float Speed = 1;

    // Once, on the first tick after the entity appears.
    public override void Start(Ctx ctx)
    {{
    }}

    // Every fixed tick (60 per second).
    public override void FixedUpdate(Ctx ctx)
    {{
    }}
}}
"#
    )
}

/// Writes a new C# script from [`cs_template`] at `scripts/<rel>` (`rel`
/// from [`new_cs_script_path`]). Refuses to replace an existing file, or a
/// class name another script already uses. Returns the file's path.
pub fn create_cs_script(project_root: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = project_root.join(SCRIPTS_DIR).join(rel);
    if path.exists() {
        return Err(format!("{SCRIPTS_DIR}/{rel} already exists"));
    }
    let kind = cs_kind_for(rel);
    if let Some(other) = find_cs_scripts(project_root)
        .into_iter()
        .find(|r| cs_kind_for(r) == kind)
    {
        return Err(format!(
            "{SCRIPTS_DIR}/{other} already makes the behaviour {kind}; pick another class name"
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, cs_template(rel))
        .map_err(|e| format!("could not write {SCRIPTS_DIR}/{rel}: {e}"))?;
    Ok(path)
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

/// A hash of everything a build depends on: the engine's API sources and
/// version, and every script's path and text.
pub fn stamp(project_root: &Path, scripts: &[String]) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    fnv(&mut h, env!("CARGO_PKG_VERSION").as_bytes());
    for (name, text) in API_SOURCES {
        fnv(&mut h, name.as_bytes());
        fnv(&mut h, text.as_bytes());
    }
    for rel in scripts {
        fnv(&mut h, rel.as_bytes());
        fnv(&mut h, &[0]);
        let text = std::fs::read(project_root.join(SCRIPTS_DIR).join(rel)).unwrap_or_default();
        fnv(&mut h, &text);
        fnv(&mut h, &[0]);
    }
    format!("{h:016x}")
}

/// The assembly of the last successful build, if it is there.
pub fn current_assembly(project_root: &Path) -> Option<PathBuf> {
    let dir = project_root.join(BUILD_DIR);
    let name = std::fs::read_to_string(dir.join(CURRENT_FILE)).ok()?;
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return None;
    }
    let dll = dir.join("out").join(name).join(format!("{ASSEMBLY}.dll"));
    dll.is_file().then_some(dll)
}

/// The files an export needs to run the built scripts, as
/// `(project-relative path, source)`.
pub fn export_files(project_root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Some(dll) = current_assembly(project_root) else {
        return Vec::new();
    };
    let mut files = vec![(
        Path::new(BUILD_DIR).join(CURRENT_FILE),
        project_root.join(BUILD_DIR).join(CURRENT_FILE),
    )];
    if let Some(dir) = dll.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file()
                && let Ok(rel) = path.strip_prefix(project_root)
            {
                files.push((rel.to_path_buf(), path.clone()));
            }
        }
    }
    files.sort();
    files
}

/// The `dotnet` command: `DOTNET_ROOT/dotnet`, else `dotnet` on `PATH`.
pub fn dotnet_command() -> PathBuf {
    let exe = if cfg!(windows) {
        "dotnet.exe"
    } else {
        "dotnet"
    };
    if let Some(root) = std::env::var_os("DOTNET_ROOT") {
        let p = PathBuf::from(root).join(exe);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from(exe)
}

/// What [`ensure_built`] found.
#[derive(Debug)]
pub enum Build {
    /// The project has no C# scripts and no built copy.
    Nothing,
    /// The assembly to load.
    Ready(PathBuf),
    /// The scripts do not compile (or .NET is missing): what to report.
    Failed(Vec<ScriptDiagnostic>),
}

fn problem(file: &str, line: Option<u32>, message: impl Into<String>) -> ScriptDiagnostic {
    ScriptDiagnostic {
        file: file.into(),
        line,
        kind: String::new(),
        entity: None,
        callback: None,
        message: message.into(),
        tick: None,
    }
}

/// The generated project file. `scripts` is reached relative to the build
/// folder so the project can move.
fn project_xml() -> String {
    format!(
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <!-- Generated by Copperframe Engine; rewritten on every build. -->
  <PropertyGroup>
    <TargetFramework>net{DOTNET_VERSION}</TargetFramework>
    <AssemblyName>{ASSEMBLY}</AssemblyName>
    <RootNamespace>Game</RootNamespace>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
    <EnableDynamicLoading>true</EnableDynamicLoading>
    <RollForward>LatestMajor</RollForward>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <DebugType>portable</DebugType>
    <Deterministic>true</Deterministic>
    <SatelliteResourceLanguages>en</SatelliteResourceLanguages>
  </PropertyGroup>
  <ItemGroup>
    <Compile Include="engine/*.cs" />
    <Compile Include="../../{SCRIPTS_DIR}/**/*.cs" Exclude="../../{SCRIPTS_DIR}/**/bin/**;../../{SCRIPTS_DIR}/**/obj/**" />
  </ItemGroup>
</Project>
"#
    )
}

/// Builds the project's C# scripts if they changed since the last build.
/// `log` receives progress lines.
pub fn ensure_built(project_root: &Path, log: &mut dyn FnMut(&str)) -> Build {
    let scripts = find_cs_scripts(project_root);
    if scripts.is_empty() {
        // An export: the built copy without sources.
        return match current_assembly(project_root) {
            Some(dll) => Build::Ready(dll),
            None => Build::Nothing,
        };
    }
    let stamp = stamp(project_root, &scripts);
    let dir = project_root.join(BUILD_DIR);
    let out = dir.join("out").join(&stamp);
    let dll = out.join(format!("{ASSEMBLY}.dll"));
    if dll.is_file() {
        set_current(&dir, &stamp);
        return Build::Ready(dll);
    }
    let _lock = match BuildLock::take(&dir) {
        Ok(l) => l,
        Err(e) => return Build::Failed(vec![problem(SCRIPTS_DIR, None, e)]),
    };
    // Another process may have built it while we waited.
    if dll.is_file() {
        set_current(&dir, &stamp);
        return Build::Ready(dll);
    }
    if let Err(e) = write_project(&dir) {
        return Build::Failed(vec![problem(SCRIPTS_DIR, None, e)]);
    }
    log("compiling C# scripts…");
    let started = Instant::now();
    let result = Command::new(dotnet_command())
        .current_dir(&dir)
        .args([
            "build",
            "-c",
            "Release",
            "-nologo",
            "-v",
            "q",
            "-clp:NoSummary",
        ])
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_NOLOGO", "1")
        .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1")
        .output();
    let output = match result {
        Ok(o) => o,
        Err(e) => {
            return Build::Failed(vec![problem(
                SCRIPTS_DIR,
                None,
                format!(
                    "C# scripts need the .NET SDK {DOTNET_VERSION} or newer, which was not \
                     found ({e}). Install it from {DOTNET_URL}, or turn C# off in Project \
                     settings > Scripting"
                ),
            )]);
        }
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut problems = parse_errors(project_root, &text);
    if !output.status.success() || !problems.is_empty() {
        if problems.is_empty() {
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let start = tail.len().saturating_sub(8);
            problems.push(problem(
                SCRIPTS_DIR,
                None,
                format!(
                    "dotnet build failed ({}):\n{}",
                    output.status,
                    tail[start..].join("\n")
                ),
            ));
        }
        return Build::Failed(problems);
    }
    let built = dir
        .join("bin")
        .join("Release")
        .join(format!("net{DOTNET_VERSION}"));
    if let Err(e) = copy_build(&built, &out) {
        return Build::Failed(vec![problem(SCRIPTS_DIR, None, e)]);
    }
    set_current(&dir, &stamp);
    prune(&dir.join("out"), &stamp);
    log(&format!(
        "compiled {} C# script(s) in {:.1} s",
        scripts.len(),
        started.elapsed().as_secs_f32()
    ));
    Build::Ready(dll)
}

fn write_project(dir: &Path) -> Result<(), String> {
    let engine = dir.join("engine");
    std::fs::create_dir_all(&engine).map_err(|e| format!("{}: {e}", engine.display()))?;
    let write = |path: PathBuf, text: &str| {
        // Unchanged files keep their time so MSBuild's incremental build works.
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    };
    write(dir.join(PROJECT_FILE), &project_xml())?;
    for (name, text) in API_SOURCES {
        write(engine.join(name), text)?;
    }
    // Keeps the IDE from treating the engine copy as the user's code.
    write(
        dir.join(".gitignore"),
        "# Generated by Copperframe Engine\n*\n",
    )?;
    Ok(())
}

fn copy_build(from: &Path, to: &Path) -> Result<(), String> {
    let tmp = to.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    for name in ["dll", "pdb", "deps.json", "runtimeconfig.json"] {
        let file = format!("{ASSEMBLY}.{name}");
        let src = from.join(&file);
        if src.is_file() {
            std::fs::copy(&src, tmp.join(&file)).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    std::fs::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()))
}

fn set_current(dir: &Path, stamp: &str) {
    let path = dir.join(CURRENT_FILE);
    if std::fs::read_to_string(&path).is_ok_and(|s| s.trim() == stamp) {
        return;
    }
    let _ = std::fs::write(path, format!("{stamp}\n"));
}

/// Removes older builds, keeping the newest few (a running game may use
/// one; a loaded assembly cannot be deleted on Windows, which is fine).
fn prune(out: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(out) else {
        return;
    };
    let mut old: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name() != keep)
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    old.sort();
    let n = old.len().saturating_sub(3);
    for (_, path) in old.into_iter().take(n) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// `path(line,col): error CS1234: message [project]` lines of a build.
pub fn parse_errors(project_root: &Path, output: &str) -> Vec<ScriptDiagnostic> {
    let root = std::path::absolute(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let mut out: Vec<ScriptDiagnostic> = Vec::new();
    for line in output.lines() {
        let Some(at) = line.find(": error ") else {
            continue;
        };
        let (place, rest) = (&line[..at], &line[at + ": error ".len()..]);
        let message = rest
            .rsplit_once(" [")
            .map_or(rest, |(m, _)| m)
            .trim()
            .to_owned();
        let (path, line_no) = match place.rsplit_once('(') {
            Some((p, pos)) => (
                p,
                pos.trim_end_matches(')')
                    .split(',')
                    .next()
                    .and_then(|n| n.parse::<u32>().ok()),
            ),
            None => (place, None),
        };
        let path = Path::new(path.trim());
        let file = path
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| {
                if path.as_os_str().is_empty() || path.ends_with(PROJECT_FILE) {
                    SCRIPTS_DIR.to_owned()
                } else {
                    path.display().to_string()
                }
            });
        let d = problem(&file, line_no, message);
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Exclusive use of the build folder while `dotnet build` runs.
struct BuildLock(PathBuf);

impl BuildLock {
    fn take(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join("build.lock");
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A lock left by a build that crashed long ago.
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().unwrap_or_default() > LOCK_WAIT * 2);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > LOCK_WAIT {
                        return Err(format!(
                            "another C# build is still running ({} exists)",
                            path.display()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
    }
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_errors_become_diagnostics() {
        let root = std::env::temp_dir().join("cf_cs_errors");
        let abs = std::path::absolute(&root).unwrap();
        let out = format!(
            "{}(7,13): error CS1002: ; expected [{}]\n\
             {0}(7,13): error CS1002: ; expected [{1}]\n\
             {}: warning CS0168: unused\n\
             MSBUILD : error MSB1009: Project file does not exist.\n",
            abs.join("scripts/Door.cs").display(),
            abs.join(".psx/csharp/CopperframeScripts.csproj").display(),
            abs.join("scripts/Door.cs").display(),
        );
        let d = parse_errors(&root, &out);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].file, "scripts/Door.cs");
        assert_eq!(d[0].line, Some(7));
        assert_eq!(d[0].message, "CS1002: ; expected");
        assert_eq!(d[1].message, "MSB1009: Project file does not exist.");
    }

    #[test]
    fn stamps_follow_the_sources_and_skip_ide_folders() {
        let root = std::env::temp_dir().join(format!("cf_cs_stamp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts/obj")).unwrap();
        std::fs::write(root.join("scripts/A.cs"), "class A {}").unwrap();
        std::fs::write(root.join("scripts/obj/Gen.cs"), "").unwrap();
        let files = find_cs_scripts(&root);
        assert_eq!(files, ["A.cs"]);
        let a = stamp(&root, &files);
        assert_eq!(a, stamp(&root, &files));
        std::fs::write(root.join("scripts/A.cs"), "class A { }").unwrap();
        assert_ne!(a, stamp(&root, &files));
        assert!(current_assembly(&root).is_none());
        assert!(matches!(
            ensure_built(&root.join("none"), &mut |_| {}),
            Build::Nothing
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_project_includes_the_engine_api_and_the_scripts() {
        let xml = project_xml();
        assert!(xml.contains("<Compile Include=\"engine/*.cs\" />"));
        assert!(xml.contains("../../scripts/**/*.cs"));
        assert!(xml.contains("<EnableDynamicLoading>true</EnableDynamicLoading>"));
    }

    #[test]
    fn snake_case_matches_the_host() {
        assert_eq!(snake_case("DoorOpener"), "door_opener");
        assert_eq!(snake_case("HTTPServer"), "http_server");
        assert_eq!(snake_case("Lever2Door"), "lever2_door");
        assert_eq!(snake_case("Spinner"), "spinner");
        assert_eq!(cs_kind_for("Puzzles/DoorOpener.cs"), "cs.door_opener");
    }

    #[test]
    fn new_cs_scripts_are_checked_and_written_once() {
        assert_eq!(new_cs_script_path(" DoorOpener ").unwrap(), "DoorOpener.cs");
        assert_eq!(
            new_cs_script_path("Puzzles\\Lever.cs").unwrap(),
            "Puzzles/Lever.cs"
        );
        assert!(new_cs_script_path("door").unwrap_err().contains("capital"));
        assert!(new_cs_script_path("My Door").is_err());
        assert!(new_cs_script_path("a/../B").is_err());
        assert!(new_cs_script_path("").is_err());
        let root = std::env::temp_dir().join(format!("cf_cs_new_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        create_cs_script(&root, "Puzzles/Lever.cs").unwrap();
        let text = std::fs::read_to_string(root.join("scripts/Puzzles/Lever.cs")).unwrap();
        assert!(text.contains("public class Lever : Behaviour"), "{text}");
        assert!(text.contains("Folder = \"Puzzles\""), "{text}");
        assert!(create_cs_script(&root, "Puzzles/Lever.cs").is_err());
        let e = create_cs_script(&root, "Lever.cs").unwrap_err();
        assert!(e.contains("already makes the behaviour cs.lever"), "{e}");
        assert_eq!(
            cs_script_for(&root, "cs.lever").as_deref(),
            Some("Puzzles/Lever.cs")
        );
        assert_eq!(cs_script_for(&root, "lua.lever"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
