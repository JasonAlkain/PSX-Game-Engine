//! The PS1 effects comparison scene.
//!
//! A small room opening onto a long corridor, built so each effect has
//! something that shows it:
//!
//! | Effect | Where to look |
//! | --- | --- |
//! | Low resolution / upscale | Everything; edges of the pillars and spheres |
//! | Vertex snapping | Edges and the spinning crate jitter as the camera moves slowly |
//! | Affine UVs | Floor and wall checkers bend near the camera, along the corridor |
//! | Colour reduction / dither | Grey gradient panel on the front-left wall, sphere shading |
//! | Fog | Pillars fading down the corridor |
//! | Materials | Cutout fence (right), blended glass pane (left), flat vs smooth spheres, unlit gradient |
//!
//! The scene is plain CPU data; the app uploads it once.

use copperframe_render::{
    AlphaMode, BlobShadow, Lighting, Material, MeshData, PointLight, TextureData,
};
use glam::{Mat4, Quat, Vec3};

/// Index into [`ComparisonScene::textures`].
pub type TextureIndex = usize;

#[derive(Debug, Clone)]
pub struct SceneObject {
    pub name: &'static str,
    pub mesh: MeshData,
    pub texture: TextureIndex,
    pub material: Material,
    pub transform: Mat4,
    /// Spins about Y (and a little about X) over time, like the Milestone 1 cube.
    pub spins: bool,
}

#[derive(Debug, Clone)]
pub struct ComparisonScene {
    pub textures: Vec<TextureData>,
    pub objects: Vec<SceneObject>,
}

pub const TEX_FLOOR: TextureIndex = 0;
pub const TEX_WALL: TextureIndex = 1;
pub const TEX_FENCE: TextureIndex = 2;
pub const TEX_WHITE: TextureIndex = 3;
pub const TEX_CRATE: TextureIndex = 4;

/// Where the camera starts: inside the room, looking down the corridor.
pub const START_POSITION: Vec3 = Vec3::new(0.0, 1.6, 5.0);
pub const START_PITCH: f32 = -0.08;

const ROOM_HALF: f32 = 6.0;
const WALL_HEIGHT: f32 = 3.0;
const CORRIDOR_HALF_WIDTH: f32 = 1.5;
const CORRIDOR_LENGTH: f32 = 40.0;

/// Brick-like pattern: offset rows of light blocks with dark mortar.
fn brick_texture() -> TextureData {
    let size = 64u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let row = y / 16;
            let xo = if row % 2 == 0 { x } else { (x + 16) % size };
            let mortar = y % 16 == 0 || xo % 32 == 0;
            // Slight per-brick variation so the colour reduction has detail.
            let shade = ((xo / 32 + row * 3) % 4) as u8 * 6;
            let px = if mortar {
                [70, 62, 58, 255]
            } else {
                [150 + shade, 92 + shade / 2, 72, 255]
            };
            rgba.extend_from_slice(&px);
        }
    }
    TextureData {
        width: size,
        height: size,
        rgba,
    }
}

/// Two-tone tiles with a thin grout line (no orientation marker).
fn floor_texture() -> TextureData {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let grout = x % 16 == 0 || y % 16 == 0;
            let dark = ((x / 16) + (y / 16)) % 2 == 1;
            rgba.extend_from_slice(&match (grout, dark) {
                (true, _) => [60, 52, 46, 255],
                (false, true) => [112, 96, 78, 255],
                (false, false) => [196, 178, 140, 255],
            });
        }
    }
    TextureData {
        width: size,
        height: size,
        rgba,
    }
}

/// Lattice with transparent holes, for alpha cutout.
fn fence_texture() -> TextureData {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let bar = x % 8 < 2 || y % 8 < 2;
            rgba.extend_from_slice(&if bar {
                [190, 190, 175, 255]
            } else {
                [0, 0, 0, 0]
            });
        }
    }
    TextureData {
        width: size,
        height: size,
        rgba,
    }
}

/// A vertical rectangle facing `yaw` (0 = +Z), centred at `centre`, with the
/// texture tiled `repeat` times. Built from a subdivided plane so affine
/// warping stays within the bounds retro hardware would have shown.
fn wall(
    width: f32,
    height: f32,
    centre: Vec3,
    yaw: f32,
    repeat: (f32, f32),
    divisions: (u32, u32),
) -> (MeshData, Mat4) {
    let mesh = MeshData::plane(width, height, divisions, repeat);
    let transform = Mat4::from_translation(centre)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2);
    (mesh, transform)
}

/// Default lighting plus a warm lamp in the corridor and a blob shadow
/// under the spinning crate (Milestone 5 local lighting).
pub fn lighting() -> Lighting {
    Lighting {
        points: vec![PointLight {
            position: Vec3::new(0.0, 1.2, -7.0),
            color: Vec3::new(1.0, 0.7, 0.35) * 1.5,
            range: 4.0,
        }],
        blobs: vec![BlobShadow {
            bottom: Vec3::new(0.0, 0.85, -2.0),
            radius: 0.6,
        }],
        ..Lighting::default()
    }
}

pub fn build() -> ComparisonScene {
    use std::f32::consts::FRAC_PI_2;

    let textures = vec![
        floor_texture(),
        brick_texture(),
        fence_texture(),
        TextureData {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
        },
        TextureData::checkerboard(64, 4, [225, 220, 205, 255], [70, 60, 80, 255]),
    ];

    let lit = Material::default();
    let mut objects = Vec::new();
    let mut add = |name, mesh, texture, material, transform| {
        objects.push(SceneObject {
            name,
            mesh,
            texture,
            material,
            transform,
            spins: false,
        });
    };

    // Floors.
    add(
        "room floor",
        MeshData::plane(2.0 * ROOM_HALF, 2.0 * ROOM_HALF, (12, 12), (6.0, 6.0)),
        TEX_FLOOR,
        lit,
        Mat4::IDENTITY,
    );
    let corridor_centre_z = -ROOM_HALF - CORRIDOR_LENGTH * 0.5;
    add(
        "corridor floor",
        MeshData::plane(
            2.0 * CORRIDOR_HALF_WIDTH,
            CORRIDOR_LENGTH,
            (3, 40),
            (1.5, 20.0),
        ),
        TEX_FLOOR,
        lit,
        Mat4::from_translation(Vec3::new(0.0, 0.0, corridor_centre_z)),
    );

    // Room walls (facing inward).
    let h = WALL_HEIGHT * 0.5;
    let side_len = 2.0 * ROOM_HALF;
    for (name, centre, yaw) in [
        ("room wall left", Vec3::new(-ROOM_HALF, h, 0.0), FRAC_PI_2),
        ("room wall right", Vec3::new(ROOM_HALF, h, 0.0), -FRAC_PI_2),
        (
            "room wall back",
            Vec3::new(0.0, h, ROOM_HALF),
            std::f32::consts::PI,
        ),
    ] {
        let (mesh, t) = wall(side_len, WALL_HEIGHT, centre, yaw, (6.0, 1.5), (6, 2));
        add(name, mesh, TEX_WALL, lit, t);
    }
    // Front wall in two segments around the corridor opening.
    let seg = ROOM_HALF - CORRIDOR_HALF_WIDTH;
    for (name, x) in [
        ("front wall left", -(CORRIDOR_HALF_WIDTH + seg * 0.5)),
        ("front wall right", CORRIDOR_HALF_WIDTH + seg * 0.5),
    ] {
        let (mesh, t) = wall(
            seg,
            WALL_HEIGHT,
            Vec3::new(x, h, -ROOM_HALF),
            0.0,
            (seg / 2.0, 1.5),
            (3, 2),
        );
        add(name, mesh, TEX_WALL, lit, t);
    }
    // Corridor walls.
    for (name, x, yaw) in [
        ("corridor wall left", -CORRIDOR_HALF_WIDTH, FRAC_PI_2),
        ("corridor wall right", CORRIDOR_HALF_WIDTH, -FRAC_PI_2),
    ] {
        let (mesh, t) = wall(
            CORRIDOR_LENGTH,
            WALL_HEIGHT,
            Vec3::new(x, h, corridor_centre_z),
            yaw,
            (CORRIDOR_LENGTH / 2.0, 1.5),
            (20, 2),
        );
        add(name, mesh, TEX_WALL, lit, t);
    }
    // Corridor end wall, deep in the fog.
    let (mesh, t) = wall(
        2.0 * CORRIDOR_HALF_WIDTH,
        WALL_HEIGHT,
        Vec3::new(0.0, h, -ROOM_HALF - CORRIDOR_LENGTH),
        0.0,
        (1.5, 1.5),
        (1, 1),
    );
    add("corridor end", mesh, TEX_WALL, lit, t);

    // Pillars down the corridor show the fog gradient.
    let mut z = -ROOM_HALF - 3.0;
    let mut left = true;
    while z > -ROOM_HALF - CORRIDOR_LENGTH + 2.0 {
        let x = if left { -1.05 } else { 1.05 };
        add(
            "corridor pillar",
            MeshData::cube(1.0),
            TEX_WALL,
            lit,
            Mat4::from_scale_rotation_translation(
                Vec3::new(0.35, WALL_HEIGHT, 0.35),
                Quat::IDENTITY,
                Vec3::new(x, h, z),
            ),
        );
        z -= 4.0;
        left = !left;
    }

    // Unlit grey gradient panel: colour reduction and dithering.
    let gradient = MeshData::plane(3.6, 0.7, (1, 1), (1.0, 1.0)).map_vertices(|v| {
        let t = if v.position[0] > 0.0 { 1.0 } else { 0.0 };
        v.color = [t, t, t, 1.0];
    });
    add(
        "gradient panel",
        gradient,
        TEX_WHITE,
        Material {
            lit: false,
            allow_affine: false,
            ..lit
        },
        Mat4::from_translation(Vec3::new(-3.75, 1.9, -ROOM_HALF + 0.02))
            * Mat4::from_rotation_x(FRAC_PI_2),
    );

    // Smooth and flat shaded spheres.
    let sphere = MeshData::sphere(0.6, 14, 9);
    add(
        "smooth sphere",
        sphere.clone(),
        TEX_WHITE,
        Material {
            tint: [0.55, 0.75, 1.0, 1.0],
            ..lit
        },
        Mat4::from_translation(Vec3::new(-2.6, 0.6, -2.5)),
    );
    add(
        "flat sphere",
        sphere,
        TEX_WHITE,
        Material {
            tint: [1.0, 0.7, 0.45, 1.0],
            flat_shading: true,
            ..lit
        },
        Mat4::from_translation(Vec3::new(2.6, 0.6, -2.5)),
    );

    // Alpha-cutout fence and an alpha-blended glass pane.
    let (mesh, t) = wall(2.4, 1.2, Vec3::new(3.3, 0.6, 0.6), -0.5, (3.0, 1.5), (2, 1));
    add(
        "cutout fence",
        mesh,
        TEX_FENCE,
        Material {
            alpha: AlphaMode::Cutout { threshold: 0.5 },
            double_sided: true,
            ..lit
        },
        t,
    );
    let (mesh, t) = wall(1.4, 1.8, Vec3::new(-3.2, 0.9, 0.6), 0.5, (1.0, 1.0), (1, 1));
    add(
        "glass pane",
        mesh,
        TEX_WHITE,
        Material {
            tint: [0.35, 0.65, 1.0, 0.35],
            alpha: AlphaMode::Blend,
            lit: false,
            double_sided: true,
            ..lit
        },
        t,
    );

    // The spinning crate from Milestone 1, last so it is easy to find.
    objects.push(SceneObject {
        name: "spinning crate",
        mesh: MeshData::cube(0.9),
        texture: TEX_CRATE,
        material: lit,
        transform: Mat4::from_translation(Vec3::new(0.0, 1.3, -2.0)),
        spins: true,
    });

    ComparisonScene { textures, objects }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_data_is_valid() {
        let scene = build();
        for t in &scene.textures {
            t.validate(4096).expect("valid texture");
        }
        for o in &scene.objects {
            o.mesh
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", o.name));
            assert!(o.texture < scene.textures.len(), "{}", o.name);
            assert!(o.transform.is_finite(), "{}", o.name);
        }
        let has = |f: fn(&SceneObject) -> bool| scene.objects.iter().any(f);
        assert!(has(|o| matches!(
            o.material.alpha,
            AlphaMode::Cutout { .. }
        )));
        assert!(has(|o| o.material.is_blended()));
        assert!(has(|o| o.material.flat_shading));
        assert!(has(|o| !o.material.lit));
        assert!(has(|o| o.spins));
    }

    #[test]
    fn walls_face_into_the_room() {
        let scene = build();
        for o in scene
            .objects
            .iter()
            .filter(|o| o.name.starts_with("room wall"))
        {
            let normal = o.transform.transform_vector3(Vec3::Y);
            let centre = o.transform.w_axis.truncate();
            // Normal points back toward the room centre.
            assert!(normal.dot(-centre.with_y(0.0)) > 0.0, "{}", o.name);
        }
    }

    // The start pose must be inside the room, checked at compile time.
    const _: () = {
        assert!(START_POSITION.x < ROOM_HALF && START_POSITION.x > -ROOM_HALF);
        assert!(START_POSITION.z < ROOM_HALF && START_POSITION.z > -ROOM_HALF);
        assert!(START_POSITION.y > 0.0 && START_POSITION.y < WALL_HEIGHT);
    };
}
