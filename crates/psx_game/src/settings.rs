//! Player-side settings and save data, stored in the user's per-game
//! directories (never in project files):
//!
//! - `settings.json` (config dir): volumes, mouse, display, key overrides.
//! - `save.json` (data dir): the [`GameState`] for "Continue".
//!
//! A missing file means defaults. A damaged file is reported, renamed to
//! `*.bad` so it is not silently overwritten, and replaced by defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::actions::Bindings;
use crate::audio::Volumes;
use crate::sim::GameState;

pub const SETTINGS_FILE: &str = "settings.json";
pub const SAVE_FILE: &str = "save.json";
const SETTINGS_FORMAT: &str = "psx_player_settings";
const SAVE_FORMAT: &str = "psx_save";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub format: String,
    pub version: u32,
    /// Linear 0..=1.
    pub master_volume: f32,
    pub music_volume: f32,
    pub effects_volume: f32,
    /// Degrees of view rotation per mouse count.
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    pub fullscreen: bool,
    pub vsync: bool,
    /// Which look to render with; the content is the same either way.
    pub graphics_style: GraphicsStyle,
    /// Key overrides as written by [`Bindings::to_json`]; empty = project
    /// defaults.
    pub bindings: Option<serde_json::Value>,
}

impl Default for UserSettings {
    fn default() -> Self {
        let v = Volumes::default();
        Self {
            format: SETTINGS_FORMAT.into(),
            version: 1,
            master_volume: v.master,
            music_volume: v.music,
            effects_volume: v.effects,
            mouse_sensitivity: 0.12,
            invert_y: false,
            fullscreen: false,
            vsync: true,
            graphics_style: GraphicsStyle::Scene,
            bindings: None,
        }
    }
}

/// Player override for the scene's retro style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsStyle {
    /// The style the scene was authored with.
    #[default]
    Scene,
    /// The PS1 preset (affine textures, snapping, nearest filtering).
    Ps1,
    /// The PS2 preset (filtered, mipmapped, per-pixel lit).
    Ps2,
}

impl GraphicsStyle {
    pub const ALL: [GraphicsStyle; 3] = [Self::Scene, Self::Ps1, Self::Ps2];

    pub fn label(self) -> &'static str {
        match self {
            Self::Scene => "As authored",
            Self::Ps1 => "PS1",
            Self::Ps2 => "PS2",
        }
    }

    /// The style to render a scene authored with `scene`: the preset's
    /// look with the scene's own fog and atmosphere (which belong to the
    /// level, not the console style).
    pub fn apply(self, scene: &psx_core::RetroStyle) -> psx_core::RetroStyle {
        let mut s = match self {
            Self::Scene => return scene.clone(),
            Self::Ps1 => psx_core::RetroStyle::ps1(),
            Self::Ps2 => psx_core::RetroStyle::ps2(),
        };
        s.fog = scene.fog;
        s.atmosphere = scene.atmosphere;
        s
    }
}

impl UserSettings {
    pub fn volumes(&self) -> Volumes {
        Volumes {
            master: self.master_volume.clamp(0.0, 1.0),
            music: self.music_volume.clamp(0.0, 1.0),
            effects: self.effects_volume.clamp(0.0, 1.0),
        }
    }

    /// Fixes out-of-range values from hand-edited files.
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        for v in [
            &mut self.master_volume,
            &mut self.music_volume,
            &mut self.effects_volume,
        ] {
            *v = if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        if !(self.mouse_sensitivity.is_finite() && (0.005..=2.0).contains(&self.mouse_sensitivity))
        {
            self.mouse_sensitivity = d.mouse_sensitivity;
        }
        self.format = d.format;
        self.version = d.version;
        self
    }

    /// Project bindings with this user's overrides applied. Invalid
    /// overrides are reported and ignored.
    pub fn bindings(&self, project: &Bindings, warnings: &mut Vec<String>) -> Bindings {
        let mut b = project.clone();
        if let Some(v) = &self.bindings {
            match Bindings::parse(&v.to_string(), "settings.json bindings") {
                Ok(user) => b.overlay(&user),
                Err(e) => warnings.push(format!("{e}; using the game's default keys")),
            }
        }
        b
    }
}

/// Where this game's settings and saves live.
#[derive(Debug, Clone)]
pub struct UserDirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl UserDirs {
    /// Per-user directories for `game` (named by its title). `None` when the
    /// platform has no home directory; the player then keeps settings in
    /// memory only and says so.
    pub fn for_game(game: &str) -> Option<UserDirs> {
        let slug = slug(game);
        let dirs = directories::ProjectDirs::from("", "PSX Engine", &slug)?;
        Some(UserDirs {
            config: dirs.config_dir().to_path_buf(),
            data: dirs.data_dir().to_path_buf(),
        })
    }

    /// Explicit directories (tests, portable installs).
    pub fn at(root: &Path) -> UserDirs {
        UserDirs {
            config: root.to_path_buf(),
            data: root.to_path_buf(),
        }
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config.join(SETTINGS_FILE)
    }

    pub fn save_path(&self) -> PathBuf {
        self.data.join(SAVE_FILE)
    }

    pub fn load_settings(&self, warnings: &mut Vec<String>) -> UserSettings {
        match read_json::<UserSettings>(&self.settings_path(), SETTINGS_FORMAT) {
            Ok(Some(s)) => s.sanitized(),
            Ok(None) => UserSettings::default(),
            Err(e) => {
                warnings.push(e);
                UserSettings::default()
            }
        }
    }

    pub fn save_settings(&self, s: &UserSettings) -> Result<(), String> {
        write_json(&self.settings_path(), s)
    }

    pub fn has_save(&self) -> bool {
        self.save_path().is_file()
    }

    pub fn load_save(&self, warnings: &mut Vec<String>) -> Option<SaveData> {
        match read_json::<SaveData>(&self.save_path(), SAVE_FORMAT) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(e);
                None
            }
        }
    }

    pub fn write_save(&self, s: &SaveData) -> Result<(), String> {
        write_json(&self.save_path(), s)
    }
}

/// Progress for "Continue": the scene to resume and the game state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveData {
    pub format: String,
    pub version: u32,
    /// Project-relative scene path.
    pub scene: String,
    pub state: GameState,
}

impl SaveData {
    pub fn new(scene: String, state: GameState) -> Self {
        Self {
            format: SAVE_FORMAT.into(),
            version: 1,
            scene,
            state,
        }
    }
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "game".into() } else { s }
}

/// `Ok(None)` when the file does not exist. On a parse error the file is
/// moved aside to `<name>.bad`.
fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    format: &str,
) -> Result<Option<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let parsed = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| e.to_string())
        .and_then(|v| {
            if v.get("format").and_then(|f| f.as_str()) != Some(format) {
                return Err(format!("expected format `{format}`"));
            }
            serde_json::from_value::<T>(v).map_err(|e| e.to_string())
        });
    match parsed {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            let bad = path.with_extension("json.bad");
            let moved = std::fs::rename(path, &bad).is_ok();
            Err(format!(
                "{} is damaged ({e}); using defaults{}",
                path.display(),
                if moved {
                    format!(" (old file kept as {})", bad.display())
                } else {
                    String::new()
                }
            ))
        }
    }
}

/// Writes through a temporary file so a crash never leaves half a file.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("could not write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text + "\n").map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("psx_settings_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn milestone_4_settings_files_load_with_the_authored_graphics_style() {
        let dir = std::env::temp_dir().join(format!("psx_settings_m4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            include_str!("../fixtures/m4_settings.json"),
        )
        .unwrap();
        let mut warnings = Vec::new();
        let s = UserDirs::at(&dir).load_settings(&mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.graphics_style, GraphicsStyle::Scene);
        assert_eq!(s.music_volume, 0.5);
        assert!(s.invert_y);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn graphics_style_keeps_the_scene_fog_and_atmosphere() {
        let mut authored = psx_core::RetroStyle::ps1();
        authored.fog.start = 2.0;
        authored.atmosphere.background = [1, 2, 3];
        let ps2 = GraphicsStyle::Ps2.apply(&authored);
        assert_eq!(ps2.fog, authored.fog);
        assert_eq!(ps2.atmosphere, authored.atmosphere);
        assert_eq!(ps2.textures, psx_core::RetroStyle::ps2().textures);
        assert_eq!(GraphicsStyle::Scene.apply(&authored), authored);
    }

    #[test]
    fn settings_and_saves_round_trip_and_damaged_files_are_kept_aside() {
        let root = temp("rt");
        let dirs = UserDirs::at(&root);
        let mut w = Vec::new();
        assert_eq!(dirs.load_settings(&mut w), UserSettings::default());
        assert!(w.is_empty());
        let s = UserSettings {
            invert_y: true,
            music_volume: 0.25,
            ..UserSettings::default()
        };
        dirs.save_settings(&s).unwrap();
        assert_eq!(dirs.load_settings(&mut w), s);

        let mut state = GameState::new();
        state.insert("doors".into(), 2.into());
        dirs.write_save(&SaveData::new("scenes/a.scene.json".into(), state.clone()))
            .unwrap();
        assert_eq!(dirs.load_save(&mut w).unwrap().state, state);

        std::fs::write(dirs.settings_path(), "{ not json").unwrap();
        assert_eq!(dirs.load_settings(&mut w), UserSettings::default());
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(root.join("settings.json.bad").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let s = UserSettings {
            master_volume: 4.0,
            mouse_sensitivity: f32::NAN,
            ..UserSettings::default()
        }
        .sanitized();
        assert_eq!(s.master_volume, 1.0);
        assert_eq!(
            s.mouse_sensitivity,
            UserSettings::default().mouse_sensitivity
        );
        assert_eq!(slug("My Game: 2!"), "my-game--2");
    }
}
