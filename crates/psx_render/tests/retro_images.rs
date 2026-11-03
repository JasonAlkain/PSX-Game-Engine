//! Offscreen image checks for each PS1 effect, with fixed geometry, camera
//! and settings. Each test enables only the effect it checks (see
//! `common::bare_style`). Debug images are written as PPM files under
//! `target/tmp/`.

mod common;

use common::{Harness, Image, Object, bare_style, gpu_or_skip, unlit, white_texture};
use glam::{Mat4, Vec3, Vec4};
use psx_core::style::ScaleMode;
use psx_render::layout::place;
use psx_render::{AlphaMode, Camera, Material, MeshData, Rect, TextureData, wgpu};

const UNORM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SRGB: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn harness() -> Option<Harness> {
    gpu_or_skip().map(|gpu| Harness::new(gpu, UNORM))
}

fn camera_at(position: Vec3) -> Camera {
    Camera {
        position,
        ..Camera::default()
    }
}

/// Bounding box (x0, y0, x1, y1), inclusive, of pixels that differ from `bg`.
fn bbox(img: &Image, bg: [u8; 3]) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..img.height {
        for x in 0..img.width {
            if img.rgb(x, y) != bg {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

#[test]
fn renders_textured_cube_without_validation_errors() {
    let Some(gpu) = gpu_or_skip() else { return };
    let h = Harness::new(gpu, SRGB);
    let cube = h.object(
        &MeshData::cube(1.0),
        &TextureData::checkerboard(64, 8, [235, 235, 235, 255], [60, 60, 60, 255]),
        Material::default(),
        Mat4::from_rotation_y(0.6) * Mat4::from_rotation_x(0.4),
    );
    let mut style = bare_style((64, 64));
    style.atmosphere.background = [0, 0, 255];
    let img = h.render(
        &style,
        &camera_at(Vec3::new(0.0, 0.0, 2.5)),
        &[cube],
        (64, 64),
        (64, 64),
        None,
    );
    img.save_ppm("cube.ppm");

    for (x, y) in [(0, 0), (63, 0), (0, 63), (63, 63)] {
        assert_eq!(img.rgb(x, y), [0, 0, 255], "corner ({x},{y})");
    }
    assert_ne!(img.rgb(32, 32), [0, 0, 255], "cube missing at centre");
    let luma = |p: [u8; 3]| p.iter().map(|&c| u32::from(c)).sum::<u32>();
    let covered: Vec<u32> = (0..64 * 64)
        .map(|i| img.rgb(i % 64, i / 64))
        .filter(|p| *p != [0, 0, 255])
        .map(luma)
        .collect();
    assert!(covered.len() > 64 * 64 / 8, "cube too small");
    let (min, max) = (covered.iter().min().unwrap(), covered.iter().max().unwrap());
    assert!(
        max - min > 150,
        "expected checkerboard contrast, got {min}..{max}"
    );
}

/// A white rectangle whose edges fall between pixels, seen head-on.
fn snap_scene(h: &Harness) -> Vec<Object> {
    vec![h.object(
        &MeshData::quad(1.03, 0.77),
        &white_texture(),
        unlit(),
        Mat4::from_translation(Vec3::new(0.13, -0.07, 0.0)),
    )]
}

#[test]
fn vertex_snapping_aligns_edges_to_the_grid() {
    let Some(h) = harness() else { return };
    let cam = camera_at(Vec3::new(0.0, 0.0, 2.0));
    let size = (64, 64);
    let render = |enabled: bool, grid: f32, strength: f32| {
        let mut style = bare_style(size);
        style.vertex_snap.enabled = enabled;
        style.vertex_snap.grid = grid;
        style.vertex_snap.strength = strength;
        h.render(&style, &cam, &snap_scene(&h), size, size, None)
    };

    let off = render(false, 4.0, 1.0);
    let snapped = render(true, 4.0, 1.0);
    let zero_strength = render(true, 4.0, 0.0);
    off.save_ppm("snap_off.ppm");
    snapped.save_ppm("snap_grid4.ppm");

    let b_off = bbox(&off, [0, 0, 0]).expect("quad visible");
    let b_snap = bbox(&snapped, [0, 0, 0]).expect("quad visible");
    // Unsnapped edges sit at 21.3/49.9 px (x) and 23.3/44.6 px (y).
    assert_eq!(b_off, (21, 23, 49, 44));
    // Snapped to a 4-pixel grid every edge lies on a multiple of 4.
    let (x0, y0, x1, y1) = b_snap;
    for edge in [x0, y0, x1 + 1, y1 + 1] {
        assert_eq!(edge % 4, 0, "edge {edge} not on the grid: {b_snap:?}");
    }
    assert_eq!(b_snap, (20, 24, 47, 43));
    assert_eq!(zero_strength, off, "strength 0 must equal snapping off");
}

#[test]
fn snapping_survives_geometry_behind_and_beside_the_camera() {
    let Some(h) = harness() else { return };
    let floor_color = [0.5, 0.5, 0.5, 1.0];
    let floor = h.object(
        &MeshData::plane(200.0, 200.0, (20, 20), (50.0, 50.0)),
        &white_texture(),
        Material {
            tint: floor_color,
            ..unlit()
        },
        Mat4::IDENTITY,
    );
    let objects = [floor];
    let size = (64, 48);
    let mut style = bare_style(size);
    style.atmosphere.background = [0, 0, 255];
    style.vertex_snap.enabled = true;
    style.affine_uv.enabled = true;
    let floor_rgb = {
        // Linear 0.5 encoded to sRGB.
        let c = (1.055 * 0.5f32.powf(1.0 / 2.4) - 0.055) * 255.0;
        c.round() as u8
    };

    // Floor passes underneath and behind the camera: vertices with w < 0.
    for (i, (height, yaw, pitch)) in [
        (1.0, 0.0, -0.3),
        (0.3, 1.0, -0.6),
        (0.3, 2.5, -1.2),
        (2.0, 4.0, -1.55),
        (0.06, 0.3, -0.1),
        (0.001, 5.0, 0.0),
        (0.5, 0.0, 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        let cam = Camera {
            position: Vec3::new(3.7, height, -1.3),
            yaw,
            pitch,
            ..Camera::default()
        };
        let img = h.render(&style, &cam, &objects, size, size, None);
        img.save_ppm(&format!("snap_clip_{i}.ppm"));
        // Only the two scene colours may appear: no holes filled with
        // garbage from invalid vertices.
        for y in 0..size.1 {
            for x in 0..size.0 {
                let p = img.rgb(x, y);
                let is_floor = p.iter().all(|&c| c.abs_diff(floor_rgb) <= 1);
                assert!(
                    p == [0, 0, 255] || is_floor,
                    "case {i}: unexpected colour {p:?} at ({x},{y})"
                );
            }
        }
        if pitch < -0.25 && height >= 0.3 {
            for x in 0..size.0 {
                assert_ne!(
                    img.rgb(x, size.1 - 1),
                    [0, 0, 255],
                    "case {i}: floor missing at bottom ({x})"
                );
            }
        }
    }
}

/// Screen row (0 = top) at which a world point projects.
fn project_row(cam: &Camera, height: u32, p: Vec3) -> f32 {
    let clip = cam.view_projection(1.0) * Vec4::new(p.x, p.y, p.z, 1.0);
    (1.0 - clip.y / clip.w) * 0.5 * height as f32
}

#[test]
fn affine_uvs_interpolate_in_screen_space() {
    let Some(h) = harness() else { return };
    let size = (64, 64);
    let cam = Camera {
        pitch: -0.5,
        ..Camera::default()
    };
    // Floor from z = -1 (near, v = 1) to z = -9 (far, v = 0); the texture is
    // white for v < 0.5 and black below.
    let stripes = TextureData::new(1, 2, vec![255, 255, 255, 255, 0, 0, 0, 255]).unwrap();
    let floor_at = |allow_affine: bool| {
        h.object(
            &MeshData::plane(2.0, 8.0, (1, 1), (1.0, 1.0)),
            &stripes,
            Material {
                allow_affine,
                ..unlit()
            },
            Mat4::from_translation(Vec3::new(0.0, -1.0, -5.0)),
        )
    };
    let boundary_row = |img: &Image| -> u32 {
        let x = size.0 / 2;
        let mut seen_white = false;
        for y in 0..size.1 {
            let p = img.rgb(x, y);
            if p == [255, 255, 255] {
                seen_white = true;
            } else if seen_white && p == [0, 0, 0] {
                return y;
            }
        }
        panic!("no white-to-black transition in the centre column");
    };

    let near = project_row(&cam, size.1, Vec3::new(0.0, -1.0, -1.0));
    let far = project_row(&cam, size.1, Vec3::new(0.0, -1.0, -9.0));
    let perspective_expected = project_row(&cam, size.1, Vec3::new(0.0, -1.0, -5.0));
    let affine_expected = (near + far) * 0.5;
    assert!(
        (perspective_expected - affine_expected).abs() > 6.0,
        "test geometry must separate the two mappings"
    );

    let mut style = bare_style(size);
    style.atmosphere.background = [0, 0, 255];
    let perspective = h.render(&style, &cam, &[floor_at(true)], size, size, None);
    style.affine_uv.enabled = true;
    let affine = h.render(&style, &cam, &[floor_at(true)], size, size, None);
    let opted_out = h.render(&style, &cam, &[floor_at(false)], size, size, None);
    perspective.save_ppm("uv_perspective.ppm");
    affine.save_ppm("uv_affine.ppm");

    let check = |name: &str, img: &Image, expected: f32| {
        let row = boundary_row(img) as f32;
        assert!(
            (row - expected).abs() <= 1.5,
            "{name}: boundary at row {row}, expected about {expected:.1}"
        );
    };
    check("perspective", &perspective, perspective_expected);
    check("affine", &affine, affine_expected);
    check("material opted out", &opted_out, perspective_expected);
}

/// Unlit quad filling the view with a horizontal black-to-white gradient.
fn gradient_scene(h: &Harness) -> Vec<Object> {
    let mesh = MeshData::quad(6.0, 2.0).map_vertices(|v| {
        let t = if v.position[0] > 0.0 { 1.0 } else { 0.0 };
        v.color = [t, t, t, 1.0];
    });
    vec![h.object(&mesh, &white_texture(), unlit(), Mat4::IDENTITY)]
}

fn levels(bits: u32) -> Vec<u8> {
    let n = (1u32 << bits) - 1;
    (0..=n)
        .map(|k| ((k as f32 / n as f32) * 255.0).round() as u8)
        .collect()
}

#[test]
fn color_reduction_and_dither() {
    let Some(h) = harness() else { return };
    let size = (64, 16);
    let cam = camera_at(Vec3::new(0.0, 0.0, 1.0));
    let render = |reduce: bool, dither: bool| {
        let mut style = bare_style(size);
        style.color_reduction.enabled = reduce;
        style.color_reduction.bits = 5;
        style.dither.enabled = dither;
        h.render(&style, &cam, &gradient_scene(&h), size, size, None)
    };
    let smooth = render(false, false);
    let reduced = render(true, false);
    let dithered = render(true, true);
    reduced.save_ppm("gradient_5bit.ppm");
    dithered.save_ppm("gradient_5bit_dither.ppm");

    let allowed = levels(5);
    let distinct = |img: &Image| {
        let mut v: Vec<u8> = img.rgba.chunks(4).map(|p| p[0]).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    assert!(distinct(&smooth).len() > 40, "unreduced gradient is smooth");
    for img in [&reduced, &dithered] {
        for &v in &distinct(img) {
            assert!(allowed.contains(&v), "{v} is not a 5-bit level");
        }
    }
    // Without dither every row is the same; with dither rows differ, and the
    // pattern repeats every 4 internal pixels vertically.
    for y in 1..size.1 {
        for x in 0..size.0 {
            assert_eq!(reduced.px(x, y), reduced.px(x, 0));
        }
    }
    let rows_differ = (0..size.0).any(|x| dithered.px(x, 0) != dithered.px(x, 1));
    assert!(rows_differ, "dither pattern missing");
    for y in 0..size.1 - 4 {
        for x in 0..size.0 {
            assert_eq!(
                dithered.px(x, y),
                dithered.px(x, y + 4),
                "period at ({x},{y})"
            );
        }
    }
    // Dither only enabled (no reduction) still produces output.
    let dither_only = render(false, true);
    assert!(distinct(&dither_only).len() > 40);
}

#[test]
fn srgb_output_matches_unorm_output_without_double_gamma() {
    let Some(gpu) = gpu_or_skip() else { return };
    let size = (64, 16);
    let cam = camera_at(Vec3::new(0.0, 0.0, 1.0));
    let mut style = bare_style(size);
    style.color_reduction.enabled = true;
    style.dither.enabled = true;
    let unorm = Harness::new(gpu, UNORM);
    let a = unorm.render(&style, &cam, &gradient_scene(&unorm), size, size, None);
    let gpu = unorm.gpu;
    let srgb = Harness::new(gpu, SRGB);
    let b = srgb.render(&style, &cam, &gradient_scene(&srgb), size, size, None);
    for (i, (x, y)) in a.rgba.iter().zip(&b.rgba).enumerate() {
        assert!(x.abs_diff(*y) <= 1, "byte {i}: unorm {x} vs srgb {y}");
    }
}

#[test]
fn integer_upscale_letterbox_and_fit_fallback() {
    let Some(h) = harness() else { return };
    let internal = (32, 24);
    let cam = camera_at(Vec3::new(0.0, 0.0, 1.0));
    let mut style = bare_style(internal);
    style.color_reduction.enabled = true;
    style.dither.enabled = true;
    let scene = gradient_scene(&h);
    let reference = h.render(&style, &cam, &scene, internal, internal, None);

    // 100×60 window: integer scale 2, centred with bars.
    let output = (100, 60);
    let p = place(output, internal, ScaleMode::Integer);
    assert_eq!(p.integer_scale, Some(2));
    let img = h.render(&style, &cam, &scene, internal, output, Some(p.rect));
    img.save_ppm("upscale_integer.ppm");
    let r = p.rect;
    for y in 0..output.1 {
        for x in 0..output.0 {
            let inside = x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
            if inside {
                // Each output pixel shows exactly its internal pixel: the
                // dither pattern is anchored to internal pixels.
                let (ix, iy) = ((x - r.x) / 2, (y - r.y) / 2);
                assert_eq!(img.px(x, y), reference.px(ix, iy), "at ({x},{y})");
            } else {
                assert_eq!(img.rgb(x, y), [0, 0, 0], "bar at ({x},{y})");
            }
        }
    }

    // Window smaller than the internal image: aspect-correct fit.
    let output = (20, 40);
    let p = place(output, internal, ScaleMode::Integer);
    assert_eq!(p.integer_scale, None);
    assert_eq!(
        p.rect,
        Rect {
            x: 0,
            y: 12,
            width: 20,
            height: 15
        }
    );
    let img = h.render(&style, &cam, &scene, internal, output, Some(p.rect));
    for x in 0..output.0 {
        assert_eq!(img.rgb(x, 0), [0, 0, 0]);
        assert_eq!(img.rgb(x, output.1 - 1), [0, 0, 0]);
    }
    // Rightmost column inside the rect shows the bright end of the gradient.
    assert!(img.rgb(19, 20)[0] > 200);
}

#[test]
fn alpha_cutout_and_blending() {
    let Some(h) = harness() else { return };
    let size = (32, 32);
    let cam = Camera::default();
    let style = bare_style(size);
    let red = || {
        h.object(
            &MeshData::quad(4.0, 4.0),
            &white_texture(),
            Material {
                tint: [1.0, 0.0, 0.0, 1.0],
                ..unlit()
            },
            Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0)),
        )
    };
    // Left texel fully transparent, right texel opaque.
    let half_clear = TextureData::new(2, 1, vec![255, 255, 255, 0, 255, 255, 255, 255]).unwrap();
    let front = |alpha: AlphaMode| {
        h.object(
            &MeshData::quad(4.0, 4.0),
            &half_clear,
            Material { alpha, ..unlit() },
            Mat4::from_translation(Vec3::new(0.0, 0.0, -2.0)),
        )
    };

    let cut = h.render(
        &style,
        &cam,
        &[red(), front(AlphaMode::Cutout { threshold: 0.5 })],
        size,
        size,
        None,
    );
    cut.save_ppm("cutout.ppm");
    assert_eq!(cut.rgb(8, 16), [255, 0, 0], "hole shows the surface behind");
    assert_eq!(cut.rgb(24, 16), [255, 255, 255]);
    // Cutout writes depth: drawn first, it still hides what is behind it.
    let cut_first = h.render(
        &style,
        &cam,
        &[front(AlphaMode::Cutout { threshold: 0.5 }), red()],
        size,
        size,
        None,
    );
    assert_eq!(cut_first, cut);

    let opaque = h.render(
        &style,
        &cam,
        &[red(), front(AlphaMode::Opaque)],
        size,
        size,
        None,
    );
    assert_eq!(opaque.rgb(8, 16), [255, 255, 255], "opaque ignores alpha");

    // A 50 % blue pane submitted *before* the red wall is still blended over it.
    let glass = h.object(
        &MeshData::quad(4.0, 4.0),
        &white_texture(),
        Material {
            tint: [0.0, 0.0, 1.0, 0.5],
            alpha: AlphaMode::Blend,
            ..unlit()
        },
        Mat4::from_translation(Vec3::new(0.0, 0.0, -2.0)),
    );
    let blended = h.render(&style, &cam, &[glass, red()], size, size, None);
    blended.save_ppm("blend.ppm");
    let [r, g, b] = blended.rgb(16, 16);
    assert!(
        r > 170 && b > 170 && g < 5,
        "expected purple, got {r},{g},{b}"
    );
}

#[test]
fn distance_fog() {
    let Some(h) = harness() else { return };
    let size = (16, 16);
    let cam = Camera::default();
    let wall = |z: f32| {
        h.object(
            &MeshData::quad(200.0, 200.0),
            &white_texture(),
            unlit(),
            Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
        )
    };
    let mut style = bare_style(size);
    style.fog.color = [10, 200, 30];
    style.fog.start = 5.0;
    style.fog.end = 20.0;
    style.fog.enabled = true;

    let far = h.render(&style, &cam, &[wall(-30.0)], size, size, None);
    let near = h.render(&style, &cam, &[wall(-3.0)], size, size, None);
    let middle = h.render(&style, &cam, &[wall(-12.0)], size, size, None);
    style.fog.enabled = false;
    let off = h.render(&style, &cam, &[wall(-30.0)], size, size, None);

    let c = (8, 8);
    for (ch, want) in far.rgb(c.0, c.1).iter().zip([10u8, 200, 30]) {
        assert!(
            ch.abs_diff(want) <= 1,
            "fully fogged: {:?}",
            far.rgb(c.0, c.1)
        );
    }
    assert_eq!(near.rgb(c.0, c.1), [255, 255, 255], "before fog start");
    let m = middle.rgb(c.0, c.1);
    assert!(
        m[0] > 10 && m[0] < 255 && m[2] > 30 && m[2] < 255,
        "partial fog: {m:?}"
    );
    assert_eq!(off.rgb(c.0, c.1), [255, 255, 255]);
}

#[test]
fn flat_shading_gives_one_colour_per_triangle() {
    let Some(h) = harness() else { return };
    let size = (48, 48);
    let cam = camera_at(Vec3::new(0.0, 0.0, 2.6));
    let render = |flat_shading: bool| {
        let sphere = h.object(
            &MeshData::sphere(1.0, 10, 6),
            &white_texture(),
            Material {
                flat_shading,
                ..Material::default()
            },
            Mat4::IDENTITY,
        );
        h.render(&bare_style(size), &cam, &[sphere], size, size, None)
    };
    let count = |img: &Image| {
        let mut v: Vec<[u8; 3]> = (0..size.0 * size.1)
            .map(|i| img.rgb(i % size.0, i / size.0))
            .collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    let smooth = render(false);
    let flat = render(true);
    flat.save_ppm("sphere_flat.ppm");
    smooth.save_ppm("sphere_smooth.ppm");
    // 10×6 sphere has at most 100 triangles; smooth shading has many more shades.
    assert!(count(&flat) <= 101, "flat: {} colours", count(&flat));
    assert!(count(&smooth) > 2 * count(&flat));
}

#[test]
fn emissive_adds_after_lighting_and_is_fogged() {
    let Some(h) = harness() else { return };
    let size = (16, 16);
    let cam = Camera::default();
    // A lit black-tinted wall facing away from the light: only emission shows.
    let wall = |emissive: [f32; 3], z: f32| {
        h.object(
            &MeshData::quad(200.0, 200.0),
            &white_texture(),
            Material {
                tint: [0.0, 0.0, 0.0, 1.0],
                emissive,
                ..Material::default()
            },
            Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
        )
    };
    let mut style = bare_style(size);
    let dark = h.render(&style, &cam, &[wall([0.0; 3], -3.0)], size, size, None);
    assert_eq!(dark.rgb(8, 8), [0, 0, 0]);
    let glow = h.render(
        &style,
        &cam,
        &[wall([0.0, 1.0, 0.0], -3.0)],
        size,
        size,
        None,
    );
    assert_eq!(glow.rgb(8, 8), [0, 255, 0], "emission is not lit or tinted");
    style.fog.enabled = true;
    style.fog.color = [255, 0, 0];
    style.fog.start = 1.0;
    style.fog.end = 2.0;
    let fogged = h.render(
        &style,
        &cam,
        &[wall([0.0, 1.0, 0.0], -10.0)],
        size,
        size,
        None,
    );
    assert_eq!(fogged.rgb(8, 8), [255, 0, 0], "emission is fogged");
}
