//! Scene resources on a real GPU: imported models render, missing assets
//! draw a visible fallback, a failed reimport keeps the last working
//! version, and picking finds the entity under a pixel.
//!
//! Needs a graphics adapter; prints `SKIPPED` without one unless
//! `PSX_REQUIRE_GPU=1` is set.

use std::collections::HashMap;

use glam::Vec3;
use psx_assets::fixtures::Fixture;
use psx_assets::project::{AssetDatabase, Project};
use psx_core::{AssetId, RetroStyle};
use psx_render::{
    Camera, CompositeSlot, GpuContext, GpuError, Lighting, RetroRenderer, ViewLayout, wgpu,
};
use psx_runtime::scene_gpu::{AssetStatus, SceneResources};
use psx_scene::{Entity, MaterialOverride, MeshComponent, MeshSource, Scene, Transform};

fn gpu_or_skip() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::headless()) {
        Ok(gpu) => Some(gpu),
        Err(err @ GpuError::NoAdapter(_)) => {
            if std::env::var("PSX_REQUIRE_GPU").as_deref() == Ok("1") {
                panic!("PSX_REQUIRE_GPU=1 but no adapter is available: {err}");
            }
            eprintln!("SKIPPED: no graphics adapter available ({err})");
            None
        }
        Err(err) => panic!("GPU initialisation failed: {err}"),
    }
}

const SIZE: (u32, u32) = (64, 48);

fn style() -> RetroStyle {
    let mut s = RetroStyle::clean();
    s.low_res.enabled = true;
    s.low_res.width = SIZE.0;
    s.low_res.height = SIZE.1;
    s.fog.enabled = false;
    s.atmosphere.background = [0, 0, 0];
    s
}

fn camera() -> Camera {
    Camera {
        position: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        ..Camera::default()
    }
}

fn render(
    gpu: &GpuContext,
    renderer: &RetroRenderer,
    resources: &mut SceneResources,
    scene: &Scene,
) -> Vec<u8> {
    let no_poses = HashMap::new();
    let view = View {
        size: SIZE,
        style: style(),
        camera: camera(),
        lighting: Lighting::default(),
        poses: &no_poses,
    };
    render_with(gpu, renderer, resources, scene, &view)
}

struct View<'a> {
    size: (u32, u32),
    style: RetroStyle,
    camera: Camera,
    lighting: Lighting,
    poses: &'a HashMap<psx_core::EntityId, Vec<glam::Mat4>>,
}

fn render_with(
    gpu: &GpuContext,
    renderer: &RetroRenderer,
    resources: &mut SceneResources,
    scene: &Scene,
    v: &View,
) -> Vec<u8> {
    let size = v.size;
    let device = &gpu.device;
    let style = v.style.clone();
    let layout = ViewLayout::for_style(size, &style).unwrap();
    let mut view = renderer.create_view(device, size.0, size.1).unwrap();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let world = scene.world_matrices();
    let draws = resources.draws(scene, &world, v.poses);
    let items: Vec<_> = draws.iter().map(|d| d.item).collect();
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render_view(
        device,
        &gpu.queue,
        &mut encoder,
        &mut view,
        &v.camera,
        &v.lighting,
        &style,
        &items,
    );
    let target_view = target.create_view(&Default::default());
    let rect = layout.placement.rect;
    renderer.composite(
        &gpu.queue,
        &mut encoder,
        &target_view,
        size,
        &[CompositeSlot {
            view: &view,
            rect,
            scissor: rect,
            style: &style,
        }],
    );
    let bytes_per_row = (size.0 * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row * size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = readback.slice(..).get_mapped_range().unwrap();
    let mut rgba = Vec::new();
    for row in 0..size.1 {
        let start = (row * bytes_per_row) as usize;
        rgba.extend_from_slice(&mapped[start..start + (size.0 * 4) as usize]);
    }
    rgba
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * SIZE.0 + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

#[test]
fn models_fallbacks_reimport_and_picking() {
    let Some(gpu) = gpu_or_skip() else { return };
    let root = std::env::temp_dir().join(format!("psx_scene_gpu_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = Project::create(&root, "GPU").unwrap();
    let model = root.join("assets/red.glb");
    std::fs::write(
        &model,
        Fixture::textured_box([0.5; 3], [1.0, 0.0, 0.0, 1.0], None).to_glb(),
    )
    .unwrap();
    let asset = AssetDatabase::register(&project, &model).unwrap().id;
    let db = AssetDatabase::scan(&project);

    let mut scene = Scene::new("t");
    let mut red = Entity::new("red");
    red.transform = Transform::from_translation(Vec3::new(0.0, 0.0, -3.0));
    red.mesh = Some(MeshComponent {
        source: MeshSource::Model { asset },
        material: MaterialOverride {
            lit: Some(false),
            ..Default::default()
        },
    });
    let mut ghost = Entity::new("ghost");
    ghost.transform = Transform::from_translation(Vec3::new(1.4, 0.0, -3.0));
    let missing = AssetId::new();
    ghost.mesh = Some(MeshComponent {
        source: MeshSource::Model { asset: missing },
        material: MaterialOverride {
            lit: Some(false),
            ..Default::default()
        },
    });
    scene.entities = vec![red.clone(), ghost.clone()];

    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let renderer = RetroRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let mut resources = SceneResources::new(&renderer, &gpu.device, &gpu.queue);
    let messages = resources.sync(&project, &db, &scene, &renderer, &gpu.device, &gpu.queue);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(matches!(
        resources.status(asset),
        Some(AssetStatus::Loaded { .. })
    ));
    assert_eq!(resources.status(missing), Some(&AssetStatus::Missing));

    let image = render(&gpu, &renderer, &mut resources, &scene);
    let centre = pixel(&image, SIZE.0 / 2, SIZE.1 / 2);
    assert_eq!(centre, [255, 0, 0], "imported model draws its base colour");
    let magenta = (SIZE.0 / 2..SIZE.0)
        .flat_map(|x| (0..SIZE.1).map(move |y| (x, y)))
        .any(|(x, y)| pixel(&image, x, y) == [255, 0, 255]);
    assert!(magenta, "missing asset draws the magenta fallback");

    // Break the source: reimport fails, the last working model stays.
    std::fs::write(&model, b"broken").unwrap();
    let err = resources.load(&project, &db, asset, &renderer, &gpu.device, &gpu.queue);
    assert!(err.unwrap().contains("keeping the last working import"));
    assert!(matches!(
        resources.status(asset),
        Some(AssetStatus::Failed {
            kept_previous: true,
            ..
        })
    ));
    let image = render(&gpu, &renderer, &mut resources, &scene);
    assert_eq!(pixel(&image, SIZE.0 / 2, SIZE.1 / 2), [255, 0, 0]);

    // Picking through the centre of the view hits the red model 2.5 m away.
    let world = scene.world_matrices();
    let (hit, distance) = resources
        .pick(&scene, &world, Vec3::ZERO, Vec3::NEG_Z)
        .unwrap();
    assert_eq!(hit, red.id);
    assert!((distance - 2.5).abs() < 1e-4);
    let (hit, _) = resources
        .pick(&scene, &world, Vec3::new(1.4, 0.0, 0.0), Vec3::NEG_Z)
        .unwrap();
    assert_eq!(hit, ghost.id, "fallback cubes are pickable too");

    if let Some(err) = pollster::block_on(scope.pop()) {
        panic!("wgpu validation error: {err}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

fn save_ppm(name: &str, size: (u32, u32), rgba: &[u8]) {
    let mut out = format!("P6\n{} {}\n255\n", size.0, size.1).into_bytes();
    for px in rgba.chunks(4) {
        out.extend_from_slice(&px[..3]);
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if std::fs::write(&path, out).is_ok() {
        eprintln!("wrote {}", path.display());
    }
}

#[test]
fn skinned_models_draw_in_their_animated_pose() {
    let Some(gpu) = gpu_or_skip() else { return };
    let root = std::env::temp_dir().join(format!("psx_scene_gpu_skin_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = Project::create(&root, "Skin").unwrap();
    let file = root.join("assets/arm.glb");
    std::fs::write(&file, Fixture::skinned_arm().to_glb()).unwrap();
    let asset = AssetDatabase::register(&project, &file).unwrap().id;
    let db = AssetDatabase::scan(&project);
    let mut scene = Scene::new("t");
    let mut arm = Entity::new("arm");
    arm.transform = Transform::from_translation(Vec3::new(0.0, -1.0, -4.0));
    arm.mesh = Some(MeshComponent {
        source: MeshSource::Model { asset },
        material: MaterialOverride {
            lit: Some(false),
            ..Default::default()
        },
    });
    let mut animator = psx_scene::AnimatorComponent::new("bend");
    animator.looping = false;
    arm.animator = Some(animator);
    scene.entities = vec![arm.clone()];

    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let renderer = RetroRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let mut resources = SceneResources::new(&renderer, &gpu.device, &gpu.queue);
    resources.sync(&project, &db, &scene, &renderer, &gpu.device, &gpu.queue);
    assert!(resources.animation(asset).is_some());
    let rest = render(&gpu, &renderer, &mut resources, &scene);
    let poses = resources.preview_poses(&scene, 2.0);
    assert!(poses.contains_key(&arm.id));
    let view = View {
        size: SIZE,
        style: style(),
        camera: camera(),
        lighting: Lighting::default(),
        poses: &poses,
    };
    let bent = render_with(&gpu, &renderer, &mut resources, &scene, &view);
    if let Some(err) = pollster::block_on(scope.pop()) {
        panic!("wgpu validation error: {err}");
    }
    let lit = |img: &[u8], x0: u32, x1: u32, y0: u32, y1: u32| {
        (x0..x1)
            .flat_map(|x| (y0..y1).map(move |y| (x, y)))
            .filter(|&(x, y)| pixel(img, x, y) != [0, 0, 0])
            .count()
    };
    // Rest: the bar stands up through the centre. Bent 90° about +Z, the
    // upper half lies towards -X (left of centre) instead.
    let (w, h) = SIZE;
    let upper_middle = |img: &[u8]| lit(img, w / 2 - 2, w / 2 + 2, 0, h / 2 - 6);
    let left = |img: &[u8]| lit(img, 0, w / 2 - 4, h / 2 - 8, h / 2 + 2);
    assert!(upper_middle(&rest) > 0 && left(&rest) == 0);
    assert!(upper_middle(&bent) == 0 && left(&bent) > 0);
    std::fs::remove_dir_all(&root).unwrap();
}

/// The example project rendered with its authored PS1 style and with the
/// PS2 preset, from the same pose and the same content (no reimport): the
/// lamp, caretaker (mid-idle) and blob shadows are in view. Captures go to
/// `target/tmp/` for review.
#[test]
fn example_room_renders_in_both_styles_from_the_same_content() {
    let Some(gpu) = gpu_or_skip() else { return };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/room");
    let project = Project::open(&root).unwrap();
    let db = AssetDatabase::scan(&project);
    let scene = psx_scene::load_scene(&project.startup_scene_path()).unwrap();
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let renderer = RetroRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let mut resources = SceneResources::new(&renderer, &gpu.device, &gpu.queue);
    let messages = resources.sync(&project, &db, &scene, &renderer, &gpu.device, &gpu.queue);
    assert!(messages.is_empty(), "{messages:?}");
    let world = scene.world_matrices();
    let poses = resources.preview_poses(&scene, 0.75);
    assert_eq!(poses.len(), 1, "the caretaker is animated");
    let camera = Camera {
        position: Vec3::new(-0.6, 1.6, 2.2),
        yaw: -1.2,
        pitch: -0.18,
        ..Camera::default()
    };
    let size = (640, 480);
    let mut shots = Vec::new();
    for (name, style) in [
        ("room_ps1.ppm", scene.settings.style.clone()),
        ("room_ps2.ppm", RetroStyle::ps2()),
    ] {
        let view = View {
            size,
            style,
            camera,
            lighting: psx_runtime::scene_gpu::lighting_for_scene(&scene, &world),
            poses: &poses,
        };
        let image = render_with(&gpu, &renderer, &mut resources, &scene, &view);
        save_ppm(name, size, &image);
        shots.push(image);
    }
    if let Some(err) = pollster::block_on(scope.pop()) {
        panic!("wgpu validation error: {err}");
    }
    assert_ne!(shots[0], shots[1]);
}
