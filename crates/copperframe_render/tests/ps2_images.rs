//! Offscreen image checks for the Milestone 5 renderer features: texture
//! filtering and mipmaps, bounded point lights (per vertex and per pixel),
//! blob shadows, GPU skinning, and geometry/UV stability under the PS2
//! preset. Debug images are written as PPM files under `target/tmp/`.

mod common;

use common::{Harness, Image, bare_style, gpu_or_skip, unlit, white_texture};
use copperframe_core::RetroStyle;
use copperframe_core::style::TextureFilter;
use copperframe_render::{
    BlobShadow, Camera, Lighting, Material, MeshData, PointLight, TextureData, wgpu,
};
use glam::{Mat4, Quat, Vec3, Vec4};

const UNORM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn harness() -> Option<Harness> {
    gpu_or_skip().map(|gpu| Harness::new(gpu, UNORM))
}

/// Pixel (column, row) at which a world point projects, for a square view.
fn project(cam: &Camera, size: u32, p: Vec3) -> (f32, f32) {
    let clip = cam.view_projection(1.0) * Vec4::new(p.x, p.y, p.z, 1.0);
    let ndc = clip.truncate() / clip.w;
    (
        (ndc.x * 0.5 + 0.5) * size as f32,
        (1.0 - ndc.y) * 0.5 * size as f32,
    )
}

fn grey(img: &Image, x: u32, y: u32) -> u8 {
    img.rgb(x, y)[0]
}

/// A front-facing unlit quad filling most of a square view at the origin
/// camera.
fn full_quad(h: &Harness, texture: &TextureData) -> common::Object {
    h.object(
        &MeshData::quad(2.0, 2.0),
        texture,
        unlit(),
        Mat4::from_translation(Vec3::new(0.0, 0.0, -1.8)),
    )
}

#[test]
fn linear_filtering_blends_texels_and_nearest_does_not() {
    let Some(h) = harness() else { return };
    let size = (32, 32);
    let checker = TextureData::new(
        2,
        2,
        [[0, 0, 0, 255], [255; 4], [255; 4], [0, 0, 0, 255]].concat(),
    )
    .unwrap();
    let quad = full_quad(&h, &checker);
    let mut style = bare_style(size);
    let nearest = h.render(&style, &Camera::default(), &[quad], size, size, None);
    style.textures.filter = TextureFilter::Linear;
    let quad = full_quad(&h, &checker);
    let linear = h.render(&style, &Camera::default(), &[quad], size, size, None);
    nearest.save_ppm("filter_nearest.ppm");
    linear.save_ppm("filter_linear.ppm");

    let distinct = |img: &Image| {
        let mut v: Vec<u8> = (8..24)
            .flat_map(|y| (8..24).map(move |x| (x, y)))
            .map(|(x, y)| grey(img, x, y))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    assert_eq!(distinct(&nearest), [0, 255], "nearest shows only texels");
    assert!(
        distinct(&linear).len() >= 8,
        "linear filtering blends texels: {:?}",
        distinct(&linear)
    );
}

#[test]
fn mipmaps_average_distant_detail_in_linear_light() {
    let Some(h) = harness() else { return };
    // 64×64 texture of one-pixel black/white checks, drawn into ~8×8
    // pixels: each pixel covers about 8×8 texels.
    let rgba: Vec<u8> = (0..64 * 64)
        .flat_map(|i| {
            if (i % 64 + i / 64) % 2 == 0 {
                [0, 0, 0, 255]
            } else {
                [255; 4]
            }
        })
        .collect();
    let fine = TextureData::new(64, 64, rgba).unwrap();
    let size = (8, 8);
    let mut style = bare_style(size);
    let render = |style: &RetroStyle, name: &str| {
        let img = h.render(
            style,
            &Camera::default(),
            &[full_quad(&h, &fine)],
            size,
            size,
            None,
        );
        img.save_ppm(name);
        img
    };
    let no_mips = render(&style, "mip_off.ppm");
    style.textures.mipmaps = true;
    let nearest_mips = render(&style, "mip_nearest.ppm");
    style.textures.filter = TextureFilter::Linear;
    let trilinear = render(&style, "mip_trilinear.ppm");

    for y in 2..6 {
        for x in 2..6 {
            let v = grey(&no_mips, x, y);
            assert!(v == 0 || v == 255, "without mipmaps texels alias: {v}");
            // 50% linear grey is sRGB 188.
            for (name, img) in [("nearest+mips", &nearest_mips), ("trilinear", &trilinear)] {
                let v = grey(img, x, y);
                assert!(
                    (180..=196).contains(&v),
                    "{name}: pixel ({x},{y}) = {v}, expected about 188"
                );
            }
        }
    }
}

fn floor_scene(h: &Harness, divisions: u32) -> common::Object {
    h.object(
        &MeshData::plane(8.0, 8.0, (divisions, divisions), (1.0, 1.0)),
        &white_texture(),
        Material::default(),
        Mat4::IDENTITY,
    )
}

fn top_down() -> Camera {
    Camera {
        position: Vec3::new(0.0, 3.0, 0.0),
        pitch: -89f32.to_radians(),
        ..Camera::default()
    }
}

#[test]
fn point_lights_are_bounded_and_lit_per_vertex_or_per_pixel() {
    let Some(h) = harness() else { return };
    let size = (64, 64);
    let cam = top_down();
    let lighting = Lighting {
        ambient: Vec3::ZERO,
        color: Vec3::ZERO,
        points: vec![PointLight {
            position: Vec3::new(0.0, 0.5, 0.0),
            color: Vec3::ONE,
            range: 1.2,
        }],
        ..Lighting::default()
    };
    let mut style = bare_style(size);
    style.shading.per_pixel = true;
    let render = |style: &RetroStyle, divisions: u32, name: &str| {
        let img = h.render_lit(
            style,
            &cam,
            &[floor_scene(&h, divisions)],
            &lighting,
            size,
            size,
            None,
        );
        img.save_ppm(name);
        img
    };
    let per_pixel = render(&style, 1, "light_per_pixel.ppm");
    let (cx, cy) = (size.0 / 2, size.1 / 2);
    // Centre: d = 0.5, (1 - (0.5/1.2)²)² ≈ 0.68 linear ≈ sRGB 216.
    let centre = grey(&per_pixel, cx, cy);
    assert!((205..=225).contains(&centre), "per-pixel centre {centre}");
    // 1.5 m from the light's foot is outside its range: exactly dark.
    let (ex, ey) = project(&cam, size.0, Vec3::new(1.5, 0.0, 0.0));
    assert_eq!(grey(&per_pixel, ex as u32, ey as u32), 0, "beyond range");

    // Per vertex, a single quad's corners are all out of range: no light.
    style.shading.per_pixel = false;
    let coarse = render(&style, 1, "light_per_vertex_coarse.ppm");
    assert!(
        grey(&coarse, cx, cy) < 8,
        "per-vertex coarse floor stays dark"
    );
    // A finely divided floor gets vertices inside the range.
    let fine = render(&style, 32, "light_per_vertex_fine.ppm");
    assert!(grey(&fine, cx, cy) > 150, "per-vertex fine floor is lit");

    // Local lights switched off by the style.
    style.shading.per_pixel = true;
    style.shading.local_lights = false;
    let off = render(&style, 1, "light_off.ppm");
    assert_eq!(grey(&off, cx, cy), 0);
}

#[test]
fn blob_shadows_darken_lit_ground_below_casters_only() {
    let Some(h) = harness() else { return };
    let size = (64, 64);
    let cam = top_down();
    let lighting = Lighting {
        direction: Vec3::NEG_Y,
        color: Vec3::splat(0.8),
        ambient: Vec3::splat(0.2),
        blobs: vec![BlobShadow {
            bottom: Vec3::new(0.0, 0.5, 0.0),
            radius: 1.0,
        }],
        ..Lighting::default()
    };
    let mut style = bare_style(size);
    let render = |style: &RetroStyle, material: Material, name: &str| {
        let floor = h.object(
            &MeshData::plane(8.0, 8.0, (1, 1), (1.0, 1.0)),
            &white_texture(),
            material,
            Mat4::IDENTITY,
        );
        let img = h.render_lit(style, &cam, &[floor], &lighting, size, size, None);
        img.save_ppm(name);
        img
    };
    let (cx, cy) = (size.0 / 2, size.1 / 2);
    let (ex, ey) = project(&cam, size.0, Vec3::new(1.6, 0.0, 0.0));
    let on = render(&style, Material::default(), "blob_on.ppm");
    assert_eq!(grey(&on, ex as u32, ey as u32), 255, "outside the blob");
    let c = grey(&on, cx, cy);
    assert!(c < 200, "centre darkened: {c}");

    let unlit_floor = render(&style, unlit(), "blob_unlit.ppm");
    assert_eq!(grey(&unlit_floor, cx, cy), 255, "unlit surfaces get none");

    style.shadows.enabled = false;
    let off = render(&style, Material::default(), "blob_off.ppm");
    assert_eq!(grey(&off, cx, cy), 255);
}

/// Two stacked unit-width cubes (y 0..1 on joint 0, y 1..2 on joint 1).
fn two_bone_bar() -> MeshData {
    let mut mesh = MeshData::default();
    for joint in 0..2u16 {
        let cube = MeshData::cube(1.0).map_vertices(|v| {
            v.position = [
                v.position[0] * 0.3,
                v.position[1] + 0.5 + f32::from(joint),
                v.position[2] * 0.3,
            ];
            v.joints = [joint, 0, 0, 0];
            v.weights = [1.0, 0.0, 0.0, 0.0];
        });
        let base = mesh.vertices.len() as u32;
        mesh.indices.extend(cube.indices.iter().map(|i| i + base));
        mesh.vertices.extend(cube.vertices);
    }
    mesh
}

#[test]
fn skinning_moves_vertices_with_their_joints() {
    let Some(h) = harness() else { return };
    let size = (64, 64);
    let cam = Camera::default();
    let model = Mat4::from_translation(Vec3::new(0.0, -1.0, -4.0));
    let pivot = Vec3::new(0.0, 1.0, 0.0);
    let bend = Mat4::from_translation(pivot)
        * Mat4::from_quat(Quat::from_rotation_z(90f32.to_radians()))
        * Mat4::from_translation(-pivot);
    let style = bare_style(size);
    let render = |joints: Option<Vec<Mat4>>, name: &str| {
        let mut bar = h.object(&two_bone_bar(), &white_texture(), unlit(), model);
        bar.joints = joints;
        let img = h.render(&style, &cam, &[bar], size, size, None);
        img.save_ppm(name);
        img
    };
    let at = |img: &Image, world: Vec3| {
        let (x, y) = project(&cam, size.0, world);
        grey(img, x as u32, y as u32)
    };
    // Upper bone in world space: straight up from (0, 0, -4), or bent to
    // point along -x.
    let up = Vec3::new(0.0, 0.7, -4.0);
    let left = Vec3::new(-0.7, 0.0, -4.0);
    let lower = Vec3::new(0.0, -0.5, -4.0);

    let unskinned = render(None, "skin_none.ppm");
    let rest = render(Some(vec![Mat4::IDENTITY; 2]), "skin_rest.ppm");
    let bent = render(Some(vec![Mat4::IDENTITY, bend]), "skin_bent.ppm");
    assert_eq!(unskinned, rest, "identity palette equals unskinned");
    assert_eq!((at(&rest, up), at(&rest, left)), (255, 0));
    assert_eq!((at(&bent, up), at(&bent, left)), (0, 255));
    assert_eq!(at(&bent, lower), 255, "joint 0 did not move");
}

#[test]
fn ps2_preset_keeps_geometry_and_uvs_where_perspective_puts_them() {
    let Some(h) = harness() else { return };
    let size = (64, 64);
    // Floor from z = -1 (v = 1) to z = -9 (v = 0): white for v < 0.5.
    let stripes = TextureData::new(1, 2, vec![255, 255, 255, 255, 0, 0, 0, 255]).unwrap();
    let mut ps2 = RetroStyle::ps2();
    ps2.low_res.width = size.0;
    ps2.low_res.height = size.1;
    ps2.fog.enabled = false;
    ps2.atmosphere.background = [0, 0, 255];
    // Nearest sampling isolates the UV mapping from filtering.
    ps2.textures.filter = TextureFilter::Nearest;
    let mut ps1 = RetroStyle::ps1();
    ps1.low_res = ps2.low_res;
    ps1.fog.enabled = false;
    ps1.atmosphere.background = [0, 0, 255];
    ps1.color_reduction.enabled = false;
    ps1.dither.enabled = false;

    let mut worst_ps1 = 0.0f32;
    // Sub-pixel camera moves: the boundary and the far edge must track the
    // perspective projection at every step.
    for step in 0..6 {
        let cam = Camera {
            position: Vec3::new(0.0, step as f32 * 0.004, 0.0),
            pitch: -0.5,
            ..Camera::default()
        };
        let floor = || {
            h.object(
                &MeshData::plane(2.0, 8.0, (1, 1), (1.0, 1.0)),
                &stripes,
                unlit(),
                Mat4::from_translation(Vec3::new(0.0, -1.0, -5.0)),
            )
        };
        let x = size.0 / 2;
        let rows = |img: &Image| {
            let column: Vec<[u8; 3]> = (0..size.1).map(|y| img.rgb(x, y)).collect();
            let top = column.iter().position(|p| *p != [0, 0, 255]).unwrap() as f32;
            let boundary = column
                .iter()
                .position(|p| *p == [0, 0, 0])
                .expect("black half visible") as f32;
            (top, boundary)
        };
        let expected_top = project(&cam, size.1, Vec3::new(0.0, -1.0, -9.0)).1;
        let expected_boundary = project(&cam, size.1, Vec3::new(0.0, -1.0, -5.0)).1;

        let img = h.render(&ps2, &cam, &[floor()], size, size, None);
        if step == 0 {
            img.save_ppm("ps2_stability.ppm");
        }
        let (top, boundary) = rows(&img);
        assert!(
            (top - expected_top).abs() <= 1.0,
            "step {step}: far edge at {top}, expected {expected_top:.2}"
        );
        assert!(
            (boundary - expected_boundary).abs() <= 1.5,
            "step {step}: UV boundary at {boundary}, expected {expected_boundary:.2}"
        );
        let (_, b1) = rows(&h.render(&ps1, &cam, &[floor()], size, size, None));
        worst_ps1 = worst_ps1.max((b1 - expected_boundary).abs());
    }
    assert!(
        worst_ps1 > 4.0,
        "the same content under PS1 shows affine warping ({worst_ps1})"
    );
}
