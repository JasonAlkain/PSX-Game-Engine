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
///
/// - 1: Milestone 2 effects.
/// - 2: adds `textures`, `shading` and `shadows`. Version 1 files load with
///   the PS1 values for those sections (nearest, no mipmaps, per-vertex
///   lighting, blob shadows), which is how version 1 rendered.
pub const STYLE_VERSION: u32 = 2;

/// Most local (point) lights applied to one view. Further lights are
/// dropped, nearest to the camera first kept.
pub const MAX_LOCAL_LIGHTS: usize = 8;
/// Most blob shadows drawn in one view (nearest to the camera kept).
pub const MAX_BLOB_SHADOWS: usize = 8;

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
    /// Added in version 2; version 1 files get the PS1 values.
    #[serde(default = "Textures::ps1")]
    pub textures: Textures,
    #[serde(default = "Shading::ps1")]
    pub shading: Shading,
    #[serde(default = "Shadows::ps1")]
    pub shadows: Shadows,
}

/// How textures are sampled. Every texture is uploaded with a full mip
/// chain, so switching this never needs content to be rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Textures {
    pub filter: TextureFilter,
    /// Use smaller mip levels for distant or angled surfaces.
    pub mipmaps: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureFilter {
    /// Crisp texels (PS1).
    Nearest,
    /// Bilinear between texels (and between mip levels with `mipmaps`).
    Linear,
}

impl Textures {
    pub fn ps1() -> Self {
        Self {
            filter: TextureFilter::Nearest,
            mipmaps: false,
        }
    }
}

/// Where lighting is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shading {
    /// Light every pixel instead of every vertex. Per-vertex lighting gives
    /// the PS1 look (local lights show the mesh's triangles); per-pixel
    /// gives smooth PS2-era pools of light.
    pub per_pixel: bool,
    /// Apply local (point) lights placed in the scene, up to
    /// [`MAX_LOCAL_LIGHTS`] per view.
    pub local_lights: bool,
}

impl Shading {
    pub fn ps1() -> Self {
        Self {
            per_pixel: false,
            local_lights: true,
        }
    }
}

/// Blob shadows: soft dark discs under entities marked as shadow casters,
/// drawn on upward-facing lit surfaces below them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shadows {
    pub enabled: bool,
    /// 0 = invisible, 1 = black at the centre.
    pub opacity: f32,
}

impl Shadows {
    pub fn ps1() -> Self {
        Self {
            enabled: true,
            opacity: 0.55,
        }
    }
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
    /// Distance from the camera, in metres, inside which affine warping is
    /// corrected. Triangles entirely beyond it warp like the PS1; vertices
    /// closer than it are perspective-correct, so walking up to a large
    /// polygon no longer swims the texture across it. 0.0 warps everywhere,
    /// as raw PS1 hardware did with untessellated polygons.
    #[serde(default = "AffineUv::default_correction_distance")]
    pub correction_distance: f32,
}

impl AffineUv {
    /// The correction distance used by the presets and by older files that
    /// predate the setting.
    pub const DEFAULT_CORRECTION_DISTANCE: f32 = 2.0;

    fn default_correction_distance() -> f32 {
        Self::DEFAULT_CORRECTION_DISTANCE
    }
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
    /// PS2-inspired: 640×448, no snapping or affine UVs, full colour,
    /// filtered mipmapped textures, per-pixel lighting.
    Ps2,
    /// Every retro effect off at native resolution; used as a reference image.
    Clean,
    Custom,
}

impl StylePreset {
    pub fn label(self) -> &'static str {
        match self {
            StylePreset::Ps1 => "PS1",
            StylePreset::Ps2 => "PS2",
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
            affine_uv: AffineUv {
                enabled: true,
                correction_distance: AffineUv::DEFAULT_CORRECTION_DISTANCE,
            },
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
            textures: Textures::ps1(),
            shading: Shading::ps1(),
            shadows: Shadows::ps1(),
        }
    }

    /// The PS2 preset. Scene parameters (fog, atmosphere) match
    /// [`RetroStyle::ps1`] so one scene reads well in both.
    pub fn ps2() -> Self {
        let mut s = Self::ps1();
        s.low_res = LowRes {
            enabled: true,
            width: 640,
            height: 448,
        };
        s.presentation.scaling = ScaleMode::Fit;
        s.vertex_snap.enabled = false;
        s.affine_uv.enabled = false;
        s.color_reduction = ColorReduction {
            enabled: false,
            bits: 8,
        };
        s.dither.enabled = false;
        s.textures = Textures {
            filter: TextureFilter::Linear,
            mipmaps: true,
        };
        s.shading.per_pixel = true;
        s.shadows.opacity = 0.6;
        s
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
            StylePreset::Ps2 => Some(Self::ps2()),
            StylePreset::Clean => Some(Self::clean()),
            StylePreset::Custom => None,
        }
    }

    /// Which preset this style equals, or `Custom` if it has been changed.
    pub fn preset(&self) -> StylePreset {
        if *self == Self::ps1() {
            StylePreset::Ps1
        } else if *self == Self::ps2() {
            StylePreset::Ps2
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
            ("shadows.opacity", self.shadows.opacity),
        ] {
            if !(v.is_finite() && (0.0..=1.0).contains(&v)) {
                return invalid(field, format!("{v} is outside 0..=1"));
            }
        }
        let d = self.affine_uv.correction_distance;
        if !(d.is_finite() && (0.0..=1000.0).contains(&d)) {
            return invalid(
                "affine_uv.correction_distance",
                format!("{d} is outside 0..=1000"),
            );
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
    // Version 1 lacks textures/shading/shadows; serde fills them with the
    // PS1 values (see STYLE_VERSION).
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
    crate::fsutil::atomic_write(path, text.as_bytes()).map_err(|source| StyleError::Write {
        path: path.to_path_buf(),
        source,
    })
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
        assert_eq!(RetroStyle::ps2().validate().ok(), Some(()));
        assert_eq!(RetroStyle::ps1().preset(), StylePreset::Ps1);
        assert_eq!(RetroStyle::ps2().preset(), StylePreset::Ps2);
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
    fn ps2_preset_turns_off_ps1_artifacts() {
        let s = RetroStyle::ps2();
        assert_eq!((s.low_res.width, s.low_res.height), (640, 448));
        assert!(!s.vertex_snap.enabled && !s.affine_uv.enabled);
        assert!(!s.color_reduction.enabled && !s.dither.enabled);
        assert_eq!(s.textures.filter, TextureFilter::Linear);
        assert!(s.textures.mipmaps && s.shading.per_pixel);
        // Same scene parameters, so one scene works in both styles.
        assert_eq!(s.fog, RetroStyle::ps1().fog);
        assert_eq!(s.atmosphere, RetroStyle::ps1().atmosphere);
    }

    #[test]
    fn version_1_files_migrate_to_ps1_values() {
        let text = style_to_json(&RetroStyle::ps1()).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["version"] = 1.into();
        let style = v["style"].as_object_mut().unwrap();
        style.remove("textures");
        style.remove("shading");
        style.remove("shadows");
        let old = serde_json::to_string(&v).unwrap();
        assert_eq!(
            parse_style(&old, Path::new("old.json")).unwrap(),
            RetroStyle::ps1()
        );
    }

    #[test]
    fn committed_version_1_file_loads_with_its_values_and_ps1_sections() {
        // The style of the Milestone 4 example room as a version 1 file,
        // with a changed snap grid and colour depth.
        let v1 = include_str!("../fixtures/v1.style.json");
        assert!(v1.contains("\"version\": 1") && !v1.contains("\"shading\""));
        let style = parse_style(v1, Path::new("v1.style.json")).unwrap();
        assert_eq!(style.vertex_snap.grid, 2.0);
        assert_eq!(style.color_reduction.bits, 4);
        assert_eq!(style.textures, Textures::ps1());
        assert_eq!(style.shading, Shading::ps1());
        assert_eq!(style.shadows, Shadows::ps1());
        let text = style_to_json(&style).unwrap();
        assert!(text.contains(&format!("\"version\": {STYLE_VERSION}")));
        assert_eq!(parse_style(&text, Path::new("x")).unwrap(), style);
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
        let future = text.replace("\"version\": 2", "\"version\": 99");
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
        let cases: [fn(&mut RetroStyle); 10] = [
            |s| s.shadows.opacity = 1.5,
            |s| s.shadows.opacity = f32::NAN,
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
