//! Retro style settings: the art-direction controls the renderer applies.
//!
//! Every effect is an independent switch with its own parameters. Changing a
//! style never touches mesh data, physics or game logic; it only feeds the
//! renderer.
//!
//! Colours in settings are **sRGB-encoded 8-bit** values, the numbers an
//! artist reads off a colour picker. The renderer converts them to linear.
//!
//! Settings are stored as versioned JSON (see [`save_style`] / [`load_style`]).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifies a style settings file.
pub const STYLE_FORMAT: &str = "psx_style";
/// Current style file version. Older versions are migrated on load; newer
/// versions are rejected.
pub const STYLE_VERSION: u32 = 1;

/// Largest internal render dimension accepted by validation. The renderer
/// also checks the device's texture limit.
pub const MAX_INTERNAL_DIMENSION: u32 = 4096;
pub const MIN_INTERNAL_DIMENSION: u32 = 16;

/// sRGB-encoded RGB colour, 0–255 per channel.
pub type Srgb8 = [u8; 3];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetroStyle {
    pub low_res: LowRes,
    pub presentation: Presentation,
    pub vertex_snap: VertexSnap,
    pub affine_uv: AffineUv,
    pub color_reduction: ColorReduction,
    pub dither: Dither,
    pub fog: Fog,
    pub atmosphere: Atmosphere,
}

/// Render the world into an offscreen image of this size, then upscale it.
/// When disabled the world renders at the native size of the on-screen image
/// area (same aspect ratio, letterboxed in the window).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LowRes {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
}

/// How the internal image is fitted into the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    /// Largest whole-number scale that fits, centred with black bars. If the
    /// window is smaller than the internal image, falls back to [`ScaleMode::Fit`].
    Integer,
    /// Largest aspect-correct size that fits; texels may be uneven in size.
    Fit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    pub scaling: ScaleMode,
}

/// Screen-space vertex snapping.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VertexSnap {
    pub enabled: bool,
    /// Snap grid cell size in internal pixels. 1.0 snaps to whole pixels;
    /// larger values give coarser, wobblier geometry.
    pub grid: f32,
    /// 0.0 = no snapping, 1.0 = full snapping.
    pub strength: f32,
}

/// Screen-linear (affine) texture coordinate interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffineUv {
    pub enabled: bool,
}

/// Quantise output colour to `bits` per RGB channel (in sRGB-encoded space).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorReduction {
    pub enabled: bool,
    pub bits: u8,
}

/// 4×4 ordered (Bayer) dither applied before quantisation, anchored to
/// internal pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dither {
    pub enabled: bool,
    /// Dither amplitude in quantisation steps. 1.0 spreads the pattern over
    /// one full step.
    pub strength: f32,
}

/// Linear distance fog, measured from the camera in meters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fog {
    pub enabled: bool,
    pub color: Srgb8,
    pub start: f32,
    pub end: f32,
}

/// Background colour and camera clip distances.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atmosphere {
    pub background: Srgb8,
    pub near: f32,
    pub far: f32,
}

/// Named starting points. A style that differs from every preset is `Custom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StylePreset {
    /// PS1-inspired defaults: 320×240, snapping, affine UVs, 5-bit colour, dither.
    Ps1,
    /// Every retro effect off at native resolution; used as a reference image.
    Clean,
    Custom,
}

impl StylePreset {
    pub fn label(self) -> &'static str {
        match self {
            StylePreset::Ps1 => "PS1",
            StylePreset::Clean => "Clean",
            StylePreset::Custom => "Custom",
        }
    }
}

const DEFAULT_BACKGROUND: Srgb8 = [24, 22, 32];

impl RetroStyle {
    /// The PS1 starting preset from the product brief.
    pub fn ps1() -> Self {
        Self {
            low_res: LowRes {
                enabled: true,
                width: 320,
                height: 240,
            },
            presentation: Presentation {
                scaling: ScaleMode::Integer,
            },
            vertex_snap: VertexSnap {
                enabled: true,
                grid: 1.0,
                strength: 1.0,
            },
            affine_uv: AffineUv { enabled: true },
            color_reduction: ColorReduction {
                enabled: true,
                bits: 5,
            },
            dither: Dither {
                enabled: true,
                strength: 1.0,
            },
            fog: Fog {
                enabled: true,
                color: DEFAULT_BACKGROUND,
                start: 6.0,
                end: 22.0,
            },
            atmosphere: Atmosphere {
                background: DEFAULT_BACKGROUND,
                near: 0.05,
                far: 60.0,
            },
        }
    }

    /// Same scene parameters as [`RetroStyle::ps1`] with every retro effect
    /// disabled. Fog and atmosphere are kept so the two are comparable.
    pub fn clean() -> Self {
        let mut s = Self::ps1();
        s.low_res.enabled = false;
        s.presentation.scaling = ScaleMode::Fit;
        s.vertex_snap.enabled = false;
        s.affine_uv.enabled = false;
        s.color_reduction.enabled = false;
        s.dither.enabled = false;
        s
    }

    pub fn from_preset(preset: StylePreset) -> Option<Self> {
        match preset {
            StylePreset::Ps1 => Some(Self::ps1()),
            StylePreset::Clean => Some(Self::clean()),
            StylePreset::Custom => None,
        }
    }

    /// Which preset this style equals, or `Custom` if it has been changed.
    pub fn preset(&self) -> StylePreset {
        if *self == Self::ps1() {
            StylePreset::Ps1
        } else if *self == Self::clean() {
            StylePreset::Clean
        } else {
            StylePreset::Custom
        }
    }

    /// Checks every value the renderer relies on.
    pub fn validate(&self) -> Result<(), StyleError> {
        let invalid =
            |field: &'static str, reason: String| Err(StyleError::Invalid { field, reason });
        let range = MIN_INTERNAL_DIMENSION..=MAX_INTERNAL_DIMENSION;
        for (field, v) in [
            ("low_res.width", self.low_res.width),
            ("low_res.height", self.low_res.height),
        ] {
            if !range.contains(&v) {
                return invalid(
                    field,
                    format!("{v} is outside {MIN_INTERNAL_DIMENSION}..={MAX_INTERNAL_DIMENSION}"),
                );
            }
        }
        let g = self.vertex_snap.grid;
        if !(g.is_finite() && (0.25..=64.0).contains(&g)) {
            return invalid("vertex_snap.grid", format!("{g} is outside 0.25..=64"));
        }
        for (field, v) in [
            ("vertex_snap.strength", self.vertex_snap.strength),
            ("dither.strength", self.dither.strength),
        ] {
            if !(v.is_finite() && (0.0..=1.0).contains(&v)) {
                return invalid(field, format!("{v} is outside 0..=1"));
            }
        }
        let bits = self.color_reduction.bits;
        if !(1..=8).contains(&bits) {
            return invalid("color_reduction.bits", format!("{bits} is outside 1..=8"));
        }
        let (start, end) = (self.fog.start, self.fog.end);
        if !(start.is_finite() && start >= 0.0) {
            return invalid("fog.start", format!("{start} must be finite and >= 0"));
        }
        if !(end.is_finite() && end > start) {
            return invalid("fog.end", format!("{end} must be greater than fog.start"));
        }
        let (near, far) = (self.atmosphere.near, self.atmosphere.far);
        if !(near.is_finite() && near >= 0.001) {
            return invalid("atmosphere.near", format!("{near} must be >= 0.001"));
        }
        if !(far.is_finite() && far > near && far / near <= 1.0e6) {
            return invalid(
                "atmosphere.far",
                format!("{far} must be greater than near and at most 1e6 × near"),
            );
        }
        Ok(())
    }
}

impl Default for RetroStyle {
    fn default() -> Self {
        Self::ps1()
    }
}

/// Converts one sRGB-encoded channel (0–1) to linear.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts an 8-bit sRGB colour to linear RGB (0–1).
pub fn srgb8_to_linear(c: Srgb8) -> [f32; 3] {
    c.map(|v| srgb_to_linear(f32::from(v) / 255.0))
}

#[derive(Debug, Error)]
pub enum StyleError {
    #[error("could not read style file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write style file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("style file {path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path} is not a style file (expected \"format\": \"{STYLE_FORMAT}\")")]
    WrongFormat { path: PathBuf },
    #[error(
        "style file {path} has version {found}; this build supports up to {STYLE_VERSION}. \
         Update the engine to open it."
    )]
    UnsupportedVersion { path: PathBuf, found: u64 },
    #[error("style file {path} has invalid contents: {source}")]
    Schema {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid style setting `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("in {path}: {source}")]
    InvalidInFile {
        path: PathBuf,
        source: Box<StyleError>,
    },
}

#[derive(Serialize)]
struct StyleFileOut<'a> {
    format: &'static str,
    version: u32,
    style: &'a RetroStyle,
}

/// Parses style JSON. `path` is used only for error messages.
pub fn parse_style(text: &str, path: &Path) -> Result<RetroStyle, StyleError> {
    let path_buf = || path.to_path_buf();
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|source| StyleError::Json {
            path: path_buf(),
            source,
        })?;
    if value.get("format").and_then(|f| f.as_str()) != Some(STYLE_FORMAT) {
        return Err(StyleError::WrongFormat { path: path_buf() });
    }
    let version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or(StyleError::WrongFormat { path: path_buf() })?;
    if version == 0 || version > u64::from(STYLE_VERSION) {
        return Err(StyleError::UnsupportedVersion {
            path: path_buf(),
            found: version,
        });
    }
    // Version 1 is current; older versions would be migrated here.
    let style_value = value
        .get("style")
        .cloned()
        .ok_or(StyleError::WrongFormat { path: path_buf() })?;
    let style: RetroStyle =
        serde_json::from_value(style_value).map_err(|source| StyleError::Schema {
            path: path_buf(),
            source,
        })?;
    style
        .validate()
        .map_err(|source| StyleError::InvalidInFile {
            path: path_buf(),
            source: Box::new(source),
        })?;
    Ok(style)
}

/// Serialises a validated style to pretty JSON with format/version header.
pub fn style_to_json(style: &RetroStyle) -> Result<String, StyleError> {
    style.validate()?;
    let out = StyleFileOut {
        format: STYLE_FORMAT,
        version: STYLE_VERSION,
        style,
    };
    // Serialising plain data structs cannot fail.
    let mut text = serde_json::to_string_pretty(&out).expect("style serialises");
    text.push('\n');
    Ok(text)
}

pub fn load_style(path: &Path) -> Result<RetroStyle, StyleError> {
    let text = fs::read_to_string(path).map_err(|source| StyleError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_style(&text, path)
}

/// Saves atomically: writes a temporary file next to `path`, keeps the
/// previous file as `<name>.bak`, then renames the temporary file into place.
pub fn save_style(style: &RetroStyle, path: &Path) -> Result<(), StyleError> {
    let text = style_to_json(style)?;
    let write_err = |source| StyleError::Write {
        path: path.to_path_buf(),
        source,
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| write_err(std::io::Error::other("path has no file name")))?;
    let sibling = |suffix: &str| {
        let mut name = file_name.to_os_string();
        name.push(suffix);
        path.with_file_name(name)
    };
    let tmp = sibling(".tmp");
    fs::write(&tmp, text).map_err(write_err)?;
    if path.exists() {
        fs::copy(path, sibling(".bak")).map_err(write_err)?;
    }
    fs::rename(&tmp, path).map_err(write_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("psx_style_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn presets_are_valid_and_recognised() {
        assert_eq!(RetroStyle::ps1().validate().ok(), Some(()));
        assert_eq!(RetroStyle::clean().validate().ok(), Some(()));
        assert_eq!(RetroStyle::ps1().preset(), StylePreset::Ps1);
        assert_eq!(RetroStyle::clean().preset(), StylePreset::Clean);
        let mut s = RetroStyle::ps1();
        s.color_reduction.bits = 6;
        assert_eq!(s.preset(), StylePreset::Custom);
    }

    #[test]
    fn ps1_preset_matches_brief() {
        let s = RetroStyle::ps1();
        assert_eq!((s.low_res.width, s.low_res.height), (320, 240));
        assert!(s.low_res.enabled && s.vertex_snap.enabled && s.affine_uv.enabled);
        assert!(s.color_reduction.enabled && s.dither.enabled);
        assert_eq!(s.color_reduction.bits, 5);
        assert_eq!(s.presentation.scaling, ScaleMode::Integer);
    }

    #[test]
    fn json_round_trip() {
        let mut s = RetroStyle::ps1();
        s.vertex_snap.grid = 2.0;
        s.fog.color = [10, 20, 30];
        let text = style_to_json(&s).unwrap();
        assert!(text.contains("\"format\": \"psx_style\""));
        assert_eq!(parse_style(&text, Path::new("x.json")).unwrap(), s);
    }

    #[test]
    fn rejects_future_versions_and_wrong_format() {
        let text = style_to_json(&RetroStyle::ps1()).unwrap();
        let future = text.replace("\"version\": 1", "\"version\": 99");
        assert!(matches!(
            parse_style(&future, Path::new("a.json")),
            Err(StyleError::UnsupportedVersion { found: 99, .. })
        ));
        let other = text.replace("psx_style", "something_else");
        assert!(matches!(
            parse_style(&other, Path::new("a.json")),
            Err(StyleError::WrongFormat { .. })
        ));
        assert!(matches!(
            parse_style("{", Path::new("a.json")),
            Err(StyleError::Json { .. })
        ));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_values() {
        let text = style_to_json(&RetroStyle::ps1()).unwrap();
        let typo = text.replace("\"bits\"", "\"bitz\"");
        assert!(matches!(
            parse_style(&typo, Path::new("a.json")),
            Err(StyleError::Schema { .. })
        ));
        let bad = text.replace("\"bits\": 5", "\"bits\": 0");
        let err = parse_style(&bad, Path::new("a.json")).unwrap_err();
        assert!(err.to_string().contains("color_reduction.bits"), "{err}");
        assert!(err.to_string().contains("a.json"), "{err}");
    }

    #[test]
    fn validation_catches_each_range() {
        let cases: [fn(&mut RetroStyle); 8] = [
            |s| s.low_res.width = 8,
            |s| s.low_res.height = 10_000,
            |s| s.vertex_snap.grid = f32::NAN,
            |s| s.vertex_snap.strength = 1.5,
            |s| s.dither.strength = -0.1,
            |s| s.fog.end = s.fog.start,
            |s| s.atmosphere.near = 0.0,
            |s| s.atmosphere.far = s.atmosphere.near,
        ];
        for (i, mutate) in cases.iter().enumerate() {
            let mut s = RetroStyle::ps1();
            mutate(&mut s);
            assert!(s.validate().is_err(), "case {i} accepted");
        }
    }

    #[test]
    fn save_is_atomic_and_keeps_backup() {
        let dir = temp_dir("save");
        let path = dir.join("style settings.json");
        let first = RetroStyle::ps1();
        save_style(&first, &path).unwrap();
        assert!(!dir.join("style settings.json.tmp").exists());
        let mut second = first.clone();
        second.dither.enabled = false;
        save_style(&second, &path).unwrap();
        assert_eq!(load_style(&path).unwrap(), second);
        assert_eq!(
            load_style(&dir.join("style settings.json.bak")).unwrap(),
            first
        );
        let mut bad = first.clone();
        bad.color_reduction.bits = 12;
        assert!(save_style(&bad, &path).is_err());
        assert_eq!(
            load_style(&path).unwrap(),
            second,
            "invalid save leaves file"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_error_names_path() {
        let err = load_style(Path::new("definitely/missing/style.json")).unwrap_err();
        assert!(err.to_string().contains("definitely/missing/style.json"));
    }

    #[test]
    fn srgb_conversion_endpoints() {
        assert_eq!(srgb8_to_linear([0, 255, 0]), [0.0, 1.0, 0.0]);
        let mid = srgb_to_linear(0.5);
        assert!((mid - 0.2140).abs() < 1e-3);
    }
}
