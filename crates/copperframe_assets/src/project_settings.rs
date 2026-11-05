//! Project-wide settings kept in `project.json` (version 2 and later) and
//! edited in the editor's Project Settings panel.
//!
//! Every section is optional in the file: a missing section, or a missing
//! field inside one, takes its default, so version 1 files load unchanged.
//! The player reads the display and style sections when it starts; the
//! editor and `copperframe export` read the build section.

use copperframe_core::RetroStyle;
use serde::{Deserialize, Serialize};

/// The game window, and the defaults a player starts with until they change
/// them in the game's settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplaySettings {
    /// Window size in logical pixels when the game opens windowed.
    pub width: u32,
    pub height: u32,
    /// Players start in fullscreen.
    pub fullscreen: bool,
    /// Players start with vsync on.
    pub vsync: bool,
}

impl DisplaySettings {
    pub const WIDTH_RANGE: std::ops::RangeInclusive<u32> = 320..=7680;
    pub const HEIGHT_RANGE: std::ops::RangeInclusive<u32> = 240..=4320;
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 960,
            fullscreen: false,
            vsync: true,
        }
    }
}

/// Which look the game renders with. The content is the same either way:
/// switching never reimports anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsStyle {
    /// The style each scene was authored with.
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
    pub fn apply(self, scene: &RetroStyle) -> RetroStyle {
        let mut s = match self {
            Self::Scene => return scene.clone(),
            Self::Ps1 => RetroStyle::ps1(),
            Self::Ps2 => RetroStyle::ps2(),
        };
        s.fog = scene.fog;
        s.atmosphere = scene.atmosphere;
        s
    }
}

/// The game's look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleSettings {
    /// The look players start with.
    pub default: GraphicsStyle,
    /// Whether the game's settings screen offers the PS1/PS2 choice. When
    /// false, the game always renders with `default`.
    pub player_can_change: bool,
}

impl Default for StyleSettings {
    fn default() -> Self {
        Self {
            default: GraphicsStyle::Scene,
            player_can_change: true,
        }
    }
}

/// Defaults for Build > Export Game and `copperframe export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSettings {
    /// Export folder, relative to the project folder or absolute. Empty
    /// means `dist/<project name>`.
    pub export_dir: String,
    /// After exporting, run the exported game headless from its own folder
    /// to check that it starts.
    pub check_after_export: bool,
    /// Test script (relative to the project) the check runs instead of an
    /// idle run. Empty means an idle run.
    pub check_script: String,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            export_dir: String::new(),
            check_after_export: true,
            check_script: String::new(),
        }
    }
}

/// Project-wide physics settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhysicsSettings {
    /// Acceleration applied to every dynamic body (m/s², scaled per body by
    /// its rigidbody's gravity scale).
    pub gravity: glam::Vec3,
}

/// Largest accepted gravity component (m/s²).
pub const MAX_GRAVITY: f32 = 1000.0;

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            gravity: glam::Vec3::new(0.0, -9.81, 0.0),
        }
    }
}

impl PhysicsSettings {
    pub fn problem(&self) -> Option<String> {
        if !self.gravity.is_finite() || self.gravity.abs().max_element() > MAX_GRAVITY {
            return Some(format!(
                "physics.gravity must be finite and within ±{MAX_GRAVITY} m/s²"
            ));
        }
        None
    }
}

/// A project name as a folder or file name: ASCII letters and digits, the
/// rest replaced by `_`.
pub fn file_slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_owned();
    if s.is_empty() { "game".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let d: DisplaySettings = serde_json::from_str(r#"{"width": 640}"#).unwrap();
        assert_eq!(d.width, 640);
        assert_eq!(d.height, DisplaySettings::default().height);
        let s: StyleSettings = serde_json::from_str(r#"{"default": "ps2"}"#).unwrap();
        assert_eq!(s.default, GraphicsStyle::Ps2);
        assert!(s.player_can_change);
        let b: BuildSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(b, BuildSettings::default());
        assert!(serde_json::from_str::<DisplaySettings>(r#"{"widht": 640}"#).is_err());
    }

    #[test]
    fn presets_keep_the_scenes_fog() {
        let mut authored = RetroStyle::ps1();
        authored.fog.enabled = true;
        authored.fog.start = 3.0;
        let ps2 = GraphicsStyle::Ps2.apply(&authored);
        assert_eq!(ps2.fog, authored.fog);
        assert_eq!(ps2.textures, RetroStyle::ps2().textures);
        assert_eq!(GraphicsStyle::Scene.apply(&authored), authored);
    }

    #[test]
    fn slugs() {
        assert_eq!(file_slug("Lever Room!"), "Lever_Room");
        assert_eq!(file_slug("  "), "game");
    }
}
