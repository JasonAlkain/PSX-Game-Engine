//! Writes the source art of the example room project:
//! `cargo run -p psx_assets --example make_room_assets -- examples/room/assets`
//!
//! The art is generated so the repository needs no binary authoring tools:
//! a textured crate and a pillar (GLB with embedded PNGs), a tile floor
//! texture (PNG), and the caretaker: a skinned box character with `idle`
//! and `wave` clips (GLB with vertex colours).

use std::io::Cursor;
use std::path::PathBuf;

use image::{ImageFormat, Rgba, RgbaImage};
use psx_assets::fixtures::{Fixture, RigBone, RigClip};

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

/// Unit quaternion `[x, y, z, w]` for `deg` degrees about `axis`.
fn rot(axis: [f32; 3], deg: f32) -> [f32; 4] {
    let h = deg.to_radians() * 0.5;
    [
        axis[0] * h.sin(),
        axis[1] * h.sin(),
        axis[2] * h.sin(),
        h.cos(),
    ]
}

/// A 1.85 m caretaker in overalls, facing +Z. Joints: hips (root, legs
/// attached), torso, head, arm_l, arm_r.
fn caretaker() -> Fixture {
    let overalls = [0.08, 0.16, 0.35];
    let shirt = [0.55, 0.32, 0.08];
    let skin = [0.62, 0.42, 0.3];
    let boots = [0.05, 0.04, 0.03];
    let cap = [0.45, 0.05, 0.04];
    let bones = vec![
        RigBone {
            name: "hips",
            parent: None,
            offset: [0.0, 0.92, 0.0],
            boxes: vec![
                ([0.0, 0.0, 0.0], [0.19, 0.1, 0.12], overalls),
                ([0.1, -0.42, 0.0], [0.075, 0.4, 0.085], overalls),
                ([-0.1, -0.42, 0.0], [0.075, 0.4, 0.085], overalls),
                ([0.1, -0.86, 0.04], [0.08, 0.05, 0.13], boots),
                ([-0.1, -0.86, 0.04], [0.08, 0.05, 0.13], boots),
            ],
        },
        RigBone {
            name: "torso",
            parent: Some(0),
            offset: [0.0, 0.1, 0.0],
            boxes: vec![
                ([0.0, 0.28, 0.0], [0.21, 0.28, 0.13], shirt),
                ([0.0, 0.16, 0.0], [0.17, 0.16, 0.135], overalls),
            ],
        },
        RigBone {
            name: "head",
            parent: Some(1),
            offset: [0.0, 0.58, 0.0],
            boxes: vec![
                ([0.0, 0.04, 0.0], [0.05, 0.04, 0.05], skin),
                ([0.0, 0.18, 0.0], [0.11, 0.12, 0.11], skin),
                ([0.0, 0.31, 0.02], [0.12, 0.03, 0.14], cap),
            ],
        },
        RigBone {
            name: "arm_l",
            parent: Some(1),
            offset: [0.27, 0.52, 0.0],
            boxes: vec![
                ([0.0, -0.25, 0.0], [0.055, 0.25, 0.06], shirt),
                ([0.0, -0.55, 0.0], [0.045, 0.06, 0.045], skin),
            ],
        },
        RigBone {
            name: "arm_r",
            parent: Some(1),
            offset: [-0.27, 0.52, 0.0],
            boxes: vec![
                ([0.0, -0.25, 0.0], [0.055, 0.25, 0.06], shirt),
                ([0.0, -0.55, 0.0], [0.045, 0.06, 0.045], skin),
            ],
        },
    ];
    let (x, y, z) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let idle = RigClip {
        name: "idle",
        tracks: vec![
            (
                1,
                vec![0.0, 1.5, 3.0],
                vec![rot(x, 0.0), rot(x, 3.0), rot(x, 0.0)],
            ),
            (
                2,
                vec![0.0, 0.75, 2.25, 3.0],
                vec![rot(y, 0.0), rot(y, 12.0), rot(y, -12.0), rot(y, 0.0)],
            ),
            (
                3,
                vec![0.0, 1.5, 3.0],
                vec![rot(z, 4.0), rot(z, 7.0), rot(z, 4.0)],
            ),
            (
                4,
                vec![0.0, 1.5, 3.0],
                vec![rot(z, -4.0), rot(z, -7.0), rot(z, -4.0)],
            ),
        ],
    };
    // The right arm (on -X) swings out and up, waves twice, and drops.
    let wave = RigClip {
        name: "wave",
        tracks: vec![
            (
                4,
                vec![0.0, 0.35, 0.6, 0.85, 1.1, 1.35, 1.8],
                [-4.0, -150.0, -125.0, -160.0, -125.0, -150.0, -4.0]
                    .iter()
                    .map(|&d| rot(z, d))
                    .collect(),
            ),
            (
                2,
                vec![0.0, 0.4, 1.4, 1.8],
                vec![rot(z, 0.0), rot(z, -8.0), rot(z, -8.0), rot(z, 0.0)],
            ),
        ],
    };
    Fixture::rigged_boxes(&bones, &[idle, wave])
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
    std::fs::create_dir_all(dir.join("characters"))?;
    std::fs::write(dir.join("characters/caretaker.glb"), caretaker().to_glb())?;
    println!("wrote room assets to {}", dir.display());
    Ok(())
}
