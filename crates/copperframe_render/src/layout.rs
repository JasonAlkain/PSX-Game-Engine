//! Presentation layout: where the internal image lands in the window.
//!
//! Fit policy (documented in `docs/ARCHITECTURE.md`):
//! - [`ScaleMode::Integer`]: the largest whole-number scale at which the
//!   internal image fits, centred. Every internal pixel becomes an exact
//!   N×N block. If the window is smaller than the internal image in either
//!   dimension, the policy falls back to `Fit`.
//! - [`ScaleMode::Fit`]: the largest rectangle with the internal image's
//!   aspect ratio that fits, centred. Internal pixels may map to blocks of
//!   uneven size (nearest-neighbour), including downscaling.
//!
//! The area outside the rectangle is cleared to black (letterbox/pillarbox).
//! When low-resolution rendering is off, the scene renders at the native size
//! of the fitted rectangle instead.

use copperframe_core::RetroStyle;
use copperframe_core::style::ScaleMode;

/// Axis-aligned pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The part of this rectangle between fractions `from..to` of its width.
    pub fn horizontal_slice(&self, from: f32, to: f32) -> Rect {
        let at = |f: f32| self.x + (self.width as f32 * f.clamp(0.0, 1.0)).round() as u32;
        let (x0, x1) = (at(from), at(to));
        Rect {
            x: x0,
            y: self.y,
            width: x1.saturating_sub(x0),
            height: self.height,
        }
    }
}

/// Where and at what size the internal image is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub rect: Rect,
    /// Whole-number scale factor, if every internal pixel is an exact square block.
    pub integer_scale: Option<u32>,
}

/// Places an `image`-sized picture into a `target`-sized area.
///
/// Returns an empty rectangle only when either size is zero.
pub fn place(target: (u32, u32), image: (u32, u32), mode: ScaleMode) -> Placement {
    let (tw, th) = target;
    let (iw, ih) = image;
    if tw == 0 || th == 0 || iw == 0 || ih == 0 {
        return Placement {
            rect: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            integer_scale: None,
        };
    }
    if mode == ScaleMode::Integer {
        let scale = (tw / iw).min(th / ih);
        if scale >= 1 {
            let (w, h) = (iw * scale, ih * scale);
            return Placement {
                rect: Rect {
                    x: (tw - w) / 2,
                    y: (th - h) / 2,
                    width: w,
                    height: h,
                },
                integer_scale: Some(scale),
            };
        }
    }
    // Aspect-correct fit. Compare tw/th with iw/ih using integer
    // cross-multiplication to avoid rounding drift.
    let (w, h) = if u64::from(tw) * u64::from(ih) <= u64::from(th) * u64::from(iw) {
        // Width-limited.
        let h = (u64::from(tw) * u64::from(ih) + u64::from(iw) / 2) / u64::from(iw);
        (tw, (h as u32).clamp(1, th))
    } else {
        let w = (u64::from(th) * u64::from(iw) + u64::from(ih) / 2) / u64::from(ih);
        ((w as u32).clamp(1, tw), th)
    };
    let integer_scale = (w % iw == 0 && h % ih == 0 && w / iw == h / ih).then_some(w / iw);
    Placement {
        rect: Rect {
            x: (tw - w) / 2,
            y: (th - h) / 2,
            width: w,
            height: h,
        },
        integer_scale,
    }
}

/// The internal render size and on-screen placement for a style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewLayout {
    /// Size of the scene colour target.
    pub internal: (u32, u32),
    pub placement: Placement,
}

impl ViewLayout {
    /// Lays out a view in a `target`-sized window. Returns `None` while the
    /// window has no area (minimized).
    pub fn for_style(target: (u32, u32), style: &RetroStyle) -> Option<ViewLayout> {
        let low = (style.low_res.width, style.low_res.height);
        if style.low_res.enabled {
            let placement = place(target, low, style.presentation.scaling);
            (!placement.rect.is_empty()).then_some(ViewLayout {
                internal: low,
                placement,
            })
        } else {
            // Native resolution: same aspect, rendered at the rectangle's size.
            let placement = place(target, low, ScaleMode::Fit);
            let rect = placement.rect;
            (!rect.is_empty()).then_some(ViewLayout {
                internal: (rect.width, rect.height),
                placement: Placement {
                    rect,
                    integer_scale: Some(1),
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scale_is_largest_that_fits_and_centred() {
        let p = place((1280, 720), (320, 240), ScaleMode::Integer);
        assert_eq!(p.integer_scale, Some(3));
        assert_eq!(
            p.rect,
            Rect {
                x: 160,
                y: 0,
                width: 960,
                height: 720
            }
        );
        let p = place((1920, 1080), (320, 240), ScaleMode::Integer);
        assert_eq!(p.integer_scale, Some(4));
        assert_eq!((p.rect.x, p.rect.y), (320, 60));
    }

    #[test]
    fn exact_fit_is_scale_one() {
        let p = place((320, 240), (320, 240), ScaleMode::Integer);
        assert_eq!(p.integer_scale, Some(1));
        assert_eq!((p.rect.width, p.rect.height), (320, 240));
    }

    #[test]
    fn integer_falls_back_to_fit_when_window_is_smaller() {
        let p = place((200, 400), (320, 240), ScaleMode::Integer);
        assert_eq!(p.integer_scale, None);
        assert_eq!(
            p.rect,
            Rect {
                x: 0,
                y: 125,
                width: 200,
                height: 150
            }
        );
    }

    #[test]
    fn fit_keeps_aspect_and_fills_one_axis() {
        let p = place((1000, 600), (320, 240), ScaleMode::Fit);
        assert_eq!((p.rect.width, p.rect.height), (800, 600));
        assert_eq!((p.rect.x, p.rect.y), (100, 0));
        let p = place((640, 1000), (320, 240), ScaleMode::Fit);
        assert_eq!((p.rect.width, p.rect.height), (640, 480));
        assert_eq!(p.integer_scale, Some(2));
        // Extreme aspect still yields a non-empty rectangle inside the target.
        let p = place((5000, 1), (320, 240), ScaleMode::Fit);
        assert_eq!((p.rect.width, p.rect.height), (1, 1));
        assert!(p.rect.x + p.rect.width <= 5000);
    }

    #[test]
    fn zero_sizes_give_empty_rect() {
        assert!(place((0, 100), (320, 240), ScaleMode::Fit).rect.is_empty());
        assert!(
            place((100, 100), (0, 240), ScaleMode::Integer)
                .rect
                .is_empty()
        );
    }

    #[test]
    fn rect_always_inside_target() {
        for tw in [1, 7, 159, 320, 321, 777, 1919] {
            for th in [1, 3, 120, 240, 241, 1033] {
                for mode in [ScaleMode::Integer, ScaleMode::Fit] {
                    let r = place((tw, th), (320, 240), mode).rect;
                    assert!(!r.is_empty());
                    assert!(r.x + r.width <= tw && r.y + r.height <= th, "{tw}x{th}");
                }
            }
        }
    }

    #[test]
    fn native_layout_renders_at_rect_size() {
        let mut style = RetroStyle::ps1();
        style.low_res.enabled = false;
        let l = ViewLayout::for_style((1280, 720), &style).unwrap();
        assert_eq!(l.internal, (960, 720));
        assert_eq!(l.placement.rect.width, 960);
        assert!(ViewLayout::for_style((0, 720), &style).is_none());
    }

    #[test]
    fn horizontal_slice_splits_without_gaps() {
        let r = Rect {
            x: 10,
            y: 0,
            width: 101,
            height: 5,
        };
        let a = r.horizontal_slice(0.0, 0.5);
        let b = r.horizontal_slice(0.5, 1.0);
        assert_eq!(a.x + a.width, b.x);
        assert_eq!(a.width + b.width, 101);
    }
}
