//! CPU-side texture data. Pixels are RGBA8 in sRGB encoding, rows top to bottom.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum TextureError {
    #[error("texture dimensions {width}x{height} must both be non-zero")]
    ZeroSize { width: u32, height: u32 },
    #[error("texture {width}x{height} exceeds the device limit of {max} pixels per side")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("texture {width}x{height} is too large to address in memory")]
    ByteSizeOverflow { width: u32, height: u32 },
    #[error("texture {width}x{height} expects {expected} bytes of RGBA8 data, got {actual}")]
    WrongDataLength {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
}

static SRGB_TO_LINEAR: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| copperframe_core::style::srgb_to_linear(i as f32 / 255.0))
});

fn linear_to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    /// Tightly packed sRGB RGBA8 pixels.
    pub rgba: Vec<u8>,
}

impl TextureData {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, TextureError> {
        let tex = Self {
            width,
            height,
            rgba,
        };
        tex.validate(u32::MAX)?;
        Ok(tex)
    }

    /// Checks dimensions against `max_dimension` and the data length.
    pub fn validate(&self, max_dimension: u32) -> Result<(), TextureError> {
        let (width, height) = (self.width, self.height);
        if width == 0 || height == 0 {
            return Err(TextureError::ZeroSize { width, height });
        }
        if width > max_dimension || height > max_dimension {
            return Err(TextureError::TooLarge {
                width,
                height,
                max: max_dimension,
            });
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(TextureError::ByteSizeOverflow { width, height })?;
        if self.rgba.len() != expected {
            return Err(TextureError::WrongDataLength {
                width,
                height,
                expected,
                actual: self.rgba.len(),
            });
        }
        Ok(())
    }

    /// Two-colour checkerboard with `cells` squares per side. A small marker in
    /// the top-left cell makes texture orientation visible.
    pub fn checkerboard(size: u32, cells: u32, a: [u8; 4], b: [u8; 4]) -> Self {
        let size = size.max(1);
        let cell = (size / cells.max(1)).max(1);
        let marker = [220, 40, 40, 255];
        let mut rgba = Vec::with_capacity(size as usize * size as usize * 4);
        for y in 0..size {
            for x in 0..size {
                let in_marker = x < cell / 2 && y < cell / 2;
                let px = if in_marker {
                    marker
                } else if ((x / cell) + (y / cell)).is_multiple_of(2) {
                    a
                } else {
                    b
                };
                rgba.extend_from_slice(&px);
            }
        }
        Self {
            width: size,
            height: size,
            rgba,
        }
    }

    /// Number of mip levels down to 1×1.
    pub fn mip_level_count(&self) -> u32 {
        32 - self.width.max(self.height).max(1).leading_zeros()
    }

    /// Levels 1.. of the mip chain (level 0 is `self`). Each level halves
    /// the previous one (rounding down, at least 1) with a 2×2 box filter
    /// computed in linear light, so dark/bright detail keeps its average
    /// brightness. Alpha is averaged linearly.
    pub fn mip_chain(&self) -> Vec<TextureData> {
        let mut out: Vec<TextureData> = Vec::new();
        let mut linear: Vec<[f32; 4]> = self
            .rgba
            .chunks_exact(4)
            .map(|p| {
                [
                    SRGB_TO_LINEAR[p[0] as usize],
                    SRGB_TO_LINEAR[p[1] as usize],
                    SRGB_TO_LINEAR[p[2] as usize],
                    f32::from(p[3]) / 255.0,
                ]
            })
            .collect();
        let (mut w, mut h) = (self.width as usize, self.height as usize);
        while w > 1 || h > 1 {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0.0f32; 4];
                    let mut n = 0.0;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                        let p = linear[sy * w + sx];
                        for c in 0..4 {
                            acc[c] += p[c];
                        }
                        n += 1.0;
                    }
                    next.push(acc.map(|v| v / n));
                }
            }
            let rgba = next
                .iter()
                .flat_map(|p| {
                    [
                        linear_to_srgb8(p[0]),
                        linear_to_srgb8(p[1]),
                        linear_to_srgb8(p[2]),
                        (p[3] * 255.0).round().clamp(0.0, 255.0) as u8,
                    ]
                })
                .collect();
            out.push(TextureData {
                width: nw as u32,
                height: nh as u32,
                rgba,
            });
            linear = next;
            (w, h) = (nw, nh);
        }
        out
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba.get(i..i + 4)?.try_into().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkerboard_alternates_and_marks_top_left() {
        let a = [255, 255, 255, 255];
        let b = [0, 0, 0, 255];
        let t = TextureData::checkerboard(64, 8, a, b);
        assert_eq!(t.validate(64), Ok(()));
        assert_eq!(t.pixel(0, 0), Some([220, 40, 40, 255]));
        assert_eq!(t.pixel(7, 7), Some(a));
        assert_eq!(t.pixel(8, 0), Some(b));
        assert_eq!(t.pixel(8, 8), Some(a));
        assert_eq!(t.pixel(64, 0), None);
    }

    #[test]
    fn mip_chain_halves_to_one_pixel_in_linear_light() {
        // Black/white checker of single pixels: every 2×2 block averages
        // to 50% linear grey, which is sRGB 188, not 128.
        let t = TextureData::checkerboard(8, 8, [0, 0, 0, 255], [255, 255, 255, 255]);
        let t = TextureData {
            rgba: t
                .rgba
                .chunks_exact(4)
                .enumerate()
                .flat_map(|(i, _)| {
                    let (x, y) = (i % 8, i / 8);
                    if (x + y) % 2 == 0 {
                        [0, 0, 0, 255]
                    } else {
                        [255; 4]
                    }
                })
                .collect(),
            ..t
        };
        assert_eq!(t.mip_level_count(), 4);
        let chain = t.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(sizes, [(4, 4), (2, 2), (1, 1)]);
        for m in &chain {
            assert_eq!(m.validate(64), Ok(()));
            for p in m.rgba.chunks_exact(4) {
                assert_eq!(p, [188, 188, 188, 255]);
            }
        }
        let odd = TextureData::new(5, 1, vec![255; 20]).unwrap();
        assert_eq!(odd.mip_level_count(), 3);
        let sizes: Vec<_> = odd
            .mip_chain()
            .iter()
            .map(|m| (m.width, m.height))
            .collect();
        assert_eq!(sizes, [(2, 1), (1, 1)]);
    }

    #[test]
    fn validation_reports_problems() {
        assert_eq!(
            TextureData::new(0, 4, vec![]),
            Err(TextureError::ZeroSize {
                width: 0,
                height: 4
            })
        );
        assert!(matches!(
            TextureData::new(2, 2, vec![0; 15]),
            Err(TextureError::WrongDataLength {
                expected: 16,
                actual: 15,
                ..
            })
        ));
        let huge = TextureData::new(u32::MAX, u32::MAX, vec![]);
        if usize::BITS <= 64 {
            assert!(matches!(huge, Err(TextureError::ByteSizeOverflow { .. })));
        }
        let t = TextureData::checkerboard(32, 4, [0; 4], [255; 4]);
        assert!(matches!(
            t.validate(16),
            Err(TextureError::TooLarge { max: 16, .. })
        ));
    }
}
