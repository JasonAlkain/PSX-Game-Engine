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
