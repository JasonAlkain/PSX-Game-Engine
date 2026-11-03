//! Keyboard controls for the retro style, until the editor (Milestone 3)
//! provides panels.
//!
//! The mapping is data so it can be printed as help and tested without a
//! window.

use psx_core::style::ScaleMode;
use psx_core::{RetroStyle, StylePreset};
use winit::keyboard::KeyCode;

/// What a key press asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleCommand {
    /// The style was changed in place; the message describes the change.
    Changed(String),
    /// Toggle the side-by-side comparison with effects off.
    ToggleSplit,
    Save,
    Reload,
    Help,
}

/// Key bindings shown by [`help_text`].
pub const BINDINGS: &[(&str, &str)] = &[
    ("1", "low-resolution rendering on/off"),
    ("2", "vertex snapping on/off"),
    ("3", "affine texture mapping on/off"),
    ("4", "colour reduction on/off"),
    ("5", "dithering on/off"),
    ("6", "fog on/off"),
    ("7", "integer scaling / aspect fit"),
    ("[ ]", "colour bits per channel -/+"),
    ("- =", "snap grid finer/coarser"),
    ("P", "next preset (PS1, Clean)"),
    ("C", "split view: current style | effects off"),
    ("F5", "save style settings"),
    ("F9", "reload style settings"),
    ("F1", "show this help"),
];

pub fn help_text() -> String {
    let mut s = String::from("style controls:");
    for (key, action) in BINDINGS {
        s.push_str(&format!("\n  {key:<4} {action}"));
    }
    s
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Applies the binding for `key` to `style`. Returns `None` for unbound keys.
pub fn apply_key(style: &mut RetroStyle, key: KeyCode) -> Option<StyleCommand> {
    let changed = |s: String| Some(StyleCommand::Changed(s));
    match key {
        KeyCode::Digit1 => {
            style.low_res.enabled ^= true;
            changed(format!("low resolution {}", on_off(style.low_res.enabled)))
        }
        KeyCode::Digit2 => {
            style.vertex_snap.enabled ^= true;
            changed(format!(
                "vertex snapping {}",
                on_off(style.vertex_snap.enabled)
            ))
        }
        KeyCode::Digit3 => {
            style.affine_uv.enabled ^= true;
            changed(format!("affine UVs {}", on_off(style.affine_uv.enabled)))
        }
        KeyCode::Digit4 => {
            style.color_reduction.enabled ^= true;
            changed(format!(
                "colour reduction {}",
                on_off(style.color_reduction.enabled)
            ))
        }
        KeyCode::Digit5 => {
            style.dither.enabled ^= true;
            changed(format!("dithering {}", on_off(style.dither.enabled)))
        }
        KeyCode::Digit6 => {
            style.fog.enabled ^= true;
            changed(format!("fog {}", on_off(style.fog.enabled)))
        }
        KeyCode::Digit7 => {
            style.presentation.scaling = match style.presentation.scaling {
                ScaleMode::Integer => ScaleMode::Fit,
                ScaleMode::Fit => ScaleMode::Integer,
            };
            changed(format!("scaling {:?}", style.presentation.scaling))
        }
        KeyCode::BracketLeft | KeyCode::BracketRight => {
            let bits = &mut style.color_reduction.bits;
            *bits = if key == KeyCode::BracketLeft {
                bits.saturating_sub(1).max(1)
            } else {
                (*bits + 1).min(8)
            };
            changed(format!("colour bits {}", style.color_reduction.bits))
        }
        KeyCode::Minus | KeyCode::Equal => {
            let grid = &mut style.vertex_snap.grid;
            *grid = if key == KeyCode::Minus {
                *grid * 0.5
            } else {
                *grid * 2.0
            }
            .clamp(0.25, 64.0);
            changed(format!("snap grid {} px", style.vertex_snap.grid))
        }
        KeyCode::KeyP => {
            let next = match style.preset() {
                StylePreset::Ps1 => StylePreset::Clean,
                StylePreset::Clean | StylePreset::Custom => StylePreset::Ps1,
            };
            *style = RetroStyle::from_preset(next).expect("named preset");
            changed(format!("preset {}", next.label()))
        }
        KeyCode::KeyC => Some(StyleCommand::ToggleSplit),
        KeyCode::F5 => Some(StyleCommand::Save),
        KeyCode::F9 => Some(StyleCommand::Reload),
        KeyCode::F1 => Some(StyleCommand::Help),
        _ => None,
    }
}

/// The comparison reference for `style`: same scene settings with every
/// retro effect off, at native resolution.
pub fn reference_style(style: &RetroStyle) -> RetroStyle {
    let mut r = style.clone();
    r.low_res.enabled = false;
    r.vertex_snap.enabled = false;
    r.affine_uv.enabled = false;
    r.color_reduction.enabled = false;
    r.dither.enabled = false;
    r
}

/// Short state summary for the window title.
pub fn summary(style: &RetroStyle) -> String {
    let mut parts = vec![format!("{}", style.preset().label())];
    parts.push(if style.low_res.enabled {
        format!("{}x{}", style.low_res.width, style.low_res.height)
    } else {
        "native".to_owned()
    });
    if style.vertex_snap.enabled {
        parts.push(format!("snap {}px", style.vertex_snap.grid));
    }
    if style.affine_uv.enabled {
        parts.push("affine".to_owned());
    }
    if style.color_reduction.enabled {
        parts.push(format!("{}-bit", style.color_reduction.bits));
    }
    if style.dither.enabled {
        parts.push("dither".to_owned());
    }
    if style.fog.enabled {
        parts.push("fog".to_owned());
    }
    parts.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_effect_key_toggles_only_its_effect() {
        let keys = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
        ];
        for key in keys {
            let mut s = RetroStyle::ps1();
            assert!(matches!(
                apply_key(&mut s, key),
                Some(StyleCommand::Changed(_))
            ));
            assert_ne!(s, RetroStyle::ps1(), "{key:?} changed nothing");
            assert_eq!(s.preset(), StylePreset::Custom);
            // Pressing again restores the preset.
            apply_key(&mut s, key);
            assert_eq!(s, RetroStyle::ps1(), "{key:?} is not a toggle");
        }
    }

    #[test]
    fn adjustments_stay_in_valid_ranges() {
        let mut s = RetroStyle::ps1();
        for _ in 0..20 {
            apply_key(&mut s, KeyCode::BracketLeft);
            apply_key(&mut s, KeyCode::Minus);
        }
        assert_eq!(s.color_reduction.bits, 1);
        assert_eq!(s.vertex_snap.grid, 0.25);
        s.validate().unwrap();
        for _ in 0..20 {
            apply_key(&mut s, KeyCode::BracketRight);
            apply_key(&mut s, KeyCode::Equal);
        }
        assert_eq!(s.color_reduction.bits, 8);
        assert_eq!(s.vertex_snap.grid, 64.0);
        s.validate().unwrap();
    }

    #[test]
    fn preset_key_cycles_and_resets_custom() {
        let mut s = RetroStyle::ps1();
        apply_key(&mut s, KeyCode::KeyP);
        assert_eq!(s.preset(), StylePreset::Clean);
        apply_key(&mut s, KeyCode::KeyP);
        assert_eq!(s.preset(), StylePreset::Ps1);
        s.dither.enabled = false;
        apply_key(&mut s, KeyCode::KeyP);
        assert_eq!(s.preset(), StylePreset::Ps1);
    }

    #[test]
    fn reference_keeps_scene_settings() {
        let s = RetroStyle::ps1();
        let r = reference_style(&s);
        assert_eq!(r.fog, s.fog);
        assert_eq!(r.atmosphere, s.atmosphere);
        assert!(!r.low_res.enabled && !r.vertex_snap.enabled && !r.dither.enabled);
        r.validate().unwrap();
    }

    #[test]
    fn unbound_keys_do_nothing() {
        let mut s = RetroStyle::ps1();
        assert_eq!(apply_key(&mut s, KeyCode::KeyW), None);
        assert_eq!(s, RetroStyle::ps1());
    }

    #[test]
    fn summary_mentions_enabled_effects() {
        let text = summary(&RetroStyle::ps1());
        assert!(text.starts_with("PS1 | 320x240"));
        assert!(text.contains("5-bit") && text.contains("dither"));
    }
}
