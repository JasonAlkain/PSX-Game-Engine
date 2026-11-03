//! Writes the source art of the example room project:
//! `cargo run -p psx_assets --example make_room_assets -- examples/room/assets`
//!
//! The art is generated so the repository needs no binary authoring tools:
//! a textured crate and a pillar (GLB with embedded PNGs) and a tile floor
//! texture (PNG).

use std::io::Cursor;
use std::path::PathBuf;

use image::{ImageFormat, Rgba, RgbaImage};
use psx_assets::fixtures::Fixture;

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .expect("PNG encoding of an in-memory image");
    out
}

/// Wooden planks with a dark frame.
fn crate_texture() -> RgbaImage {
    RgbaImage::from_fn(32, 32, |x, y| {
        let frame = x < 3 || y < 3 || x > 28 || y > 28 || (x as i32 - y as i32).abs() < 2;
        let grain = ((y * 7 + x / 3) % 5) as u8 * 6;
        if frame {
            Rgba([90 + grain / 2, 60, 30, 255])
        } else {
            Rgba([160 + grain, 115 + grain, 60, 255])
        }
    })
}

/// Grey stone blocks.
fn stone_texture() -> RgbaImage {
    RgbaImage::from_fn(32, 64, |x, y| {
        let row = y / 8;
        let shift = if row % 2 == 0 { 0 } else { 8 };
        let mortar = y % 8 == 0 || (x + shift) % 16 == 0;
        let n = ((x * 13 + y * 7) % 9) as u8 * 4;
        if mortar {
            Rgba([60, 58, 55, 255])
        } else {
            Rgba([130 + n, 126 + n, 118 + n, 255])
        }
    })
}

/// Checker tiles with grout.
fn floor_texture() -> RgbaImage {
    RgbaImage::from_fn(64, 64, |x, y| {
        if x % 32 == 0 || y % 32 == 0 {
            Rgba([40, 38, 36, 255])
        } else if ((x / 32) + (y / 32)) % 2 == 0 {
            Rgba([150, 40, 40, 255])
        } else {
            Rgba([200, 190, 170, 255])
        }
    })
}

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "examples/room/assets".to_owned()),
    );
    std::fs::create_dir_all(dir.join("props"))?;
    std::fs::create_dir_all(dir.join("textures"))?;
    let crate_glb =
        Fixture::textured_box([0.5, 0.5, 0.5], [1.0; 4], Some(&png(&crate_texture()))).to_glb();
    std::fs::write(dir.join("props/crate.glb"), crate_glb)?;
    let pillar_glb =
        Fixture::textured_box([0.3, 1.5, 0.3], [1.0; 4], Some(&png(&stone_texture()))).to_glb();
    std::fs::write(dir.join("props/pillar.glb"), pillar_glb)?;
    std::fs::write(dir.join("textures/floor_tiles.png"), png(&floor_texture()))?;
    println!("wrote room assets to {}", dir.display());
    Ok(())
}
