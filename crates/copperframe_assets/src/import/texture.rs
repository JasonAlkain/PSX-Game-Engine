//! PNG/JPEG decoding into sRGB RGBA8, with import-time size limits.

use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, ImageReader, Limits};

use super::{CpuTexture, ImportError, MAX_IMAGE_DIMENSION, guarded, read_source};
use crate::meta::ImportSettings;

/// Imports a standalone texture file.
pub fn import_texture(path: &Path, settings: &ImportSettings) -> Result<CpuTexture, ImportError> {
    guarded(path, || {
        let bytes = read_source(path)?;
        let (texture, _) =
            decode_image(&bytes, None, settings.max_texture_size).map_err(|message| {
                ImportError::Image {
                    path: path.to_path_buf(),
                    what: path.display().to_string(),
                    message,
                }
            })?;
        Ok(texture)
    })
}

/// Decodes PNG or JPEG bytes (format from `mime` or sniffed) into RGBA8.
/// Images larger than `max_size` on either side are reduced with nearest
/// sampling, keeping the aspect ratio. Returns the texture and, when it was
/// reduced, the original size.
pub(crate) fn decode_image(
    bytes: &[u8],
    mime: Option<&str>,
    max_size: u32,
) -> Result<(CpuTexture, Option<(u32, u32)>), String> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    match mime {
        Some("image/png") => reader.set_format(ImageFormat::Png),
        Some("image/jpeg") => reader.set_format(ImageFormat::Jpeg),
        Some(other) => {
            return Err(format!(
                "unsupported image type `{other}` (PNG and JPEG only)"
            ));
        }
        None => {
            reader = reader
                .with_guessed_format()
                .map_err(|e| format!("could not detect the image format: {e}"))?;
        }
    }
    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg) => {}
        Some(other) => {
            return Err(format!(
                "unsupported image format {other:?} (PNG and JPEG only)"
            ));
        }
        None => return Err("unknown image format (PNG and JPEG only)".to_owned()),
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?;
    let rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err("image has zero size".to_owned());
    }
    let max = max_size.max(1);
    if w <= max && h <= max {
        return Ok((
            CpuTexture {
                width: w,
                height: h,
                rgba: rgba.into_raw(),
            },
            None,
        ));
    }
    let scale = f64::from(max) / f64::from(w.max(h));
    let nw = ((f64::from(w) * scale).round() as u32).clamp(1, max);
    let nh = ((f64::from(h) * scale).round() as u32).clamp(1, max);
    let resized = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Nearest);
    Ok((
        CpuTexture {
            width: nw,
            height: nh,
            rgba: resized.into_raw(),
        },
        Some((w, h)),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Encodes a small RGBA PNG for fixtures.
    pub fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_and_limits_size() {
        let bytes = png(8, 4, |x, _| [x as u8 * 30, 0, 0, 255]);
        let (t, reduced) = decode_image(&bytes, None, 1024).unwrap();
        assert_eq!((t.width, t.height, reduced), (8, 4, None));
        assert_eq!(&t.rgba[4..8], &[30, 0, 0, 255]);
        let (t, reduced) = decode_image(&bytes, Some("image/png"), 4).unwrap();
        assert_eq!((t.width, t.height, reduced), (4, 2, Some((8, 4))));
        assert_eq!(t.rgba.len(), 4 * 2 * 4);
    }

    #[test]
    fn rejects_garbage_and_other_formats() {
        assert!(decode_image(b"not an image", None, 64).is_err());
        assert!(decode_image(&png(2, 2, |_, _| [0; 4]), Some("image/webp"), 64).is_err());
        // Truncated PNG.
        let bytes = png(16, 16, |_, _| [9; 4]);
        assert!(decode_image(&bytes[..bytes.len() / 2], None, 64).is_err());
    }
}
