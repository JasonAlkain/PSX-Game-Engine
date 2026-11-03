//! Renders the comparison scene from many camera poses (inside walls, below
//! the floor, grazing the near plane, looking straight up/down) with every
//! PS1 effect on, and fails on any GPU validation error. Also writes PS1 and
//! effects-off captures from the start pose to `target/tmp/` for review.
//!
//! Needs a graphics adapter; prints `SKIPPED` without one unless
//! `PSX_REQUIRE_GPU=1` is set.

use glam::{Mat4, Vec3};
use psx_core::RetroStyle;
use psx_render::{
    Camera, CompositeSlot, DrawItem, GpuContext, GpuError, Lighting, RetroRenderer, ViewLayout,
    wgpu,
};
use psx_runtime::comparison_scene;
use psx_runtime::style_controls::reference_style;

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

struct Scene {
    renderer: RetroRenderer,
    textures: Vec<psx_render::GpuTexture>,
    objects: Vec<(psx_render::GpuMesh, usize, psx_render::Material, Mat4)>,
}

fn upload(gpu: &GpuContext) -> Scene {
    let renderer = RetroRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let scene = comparison_scene::build();
    let textures = scene
        .textures
        .iter()
        .map(|t| renderer.upload_texture(&gpu.device, &gpu.queue, t).unwrap())
        .collect();
    let objects = scene
        .objects
        .iter()
        .map(|o| {
            let mesh = renderer.upload_mesh(&gpu.device, &o.mesh).unwrap();
            (mesh, o.texture, o.material, o.transform)
        })
        .collect();
    Scene {
        renderer,
        textures,
        objects,
    }
}

/// Renders one frame into an `output`-sized Rgba8Unorm image and returns the
/// pixels (tightly packed rows).
fn render(
    gpu: &GpuContext,
    scene: &Scene,
    style: &RetroStyle,
    camera: &Camera,
    output: (u32, u32),
    split: bool,
) -> Vec<u8> {
    let device = &gpu.device;
    let layout = ViewLayout::for_style(output, style).unwrap();
    let rect = layout.placement.rect;
    let mut view = scene
        .renderer
        .create_view(device, layout.internal.0, layout.internal.1)
        .unwrap();
    let mut reference = scene
        .renderer
        .create_view(device, rect.width, rect.height)
        .unwrap();
    let reference_style = reference_style(style);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d {
            width: output.0,
            height: output.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let items: Vec<DrawItem<'_>> = scene
        .objects
        .iter()
        .map(|(mesh, tex, material, model)| DrawItem {
            joints: None,
            mesh,
            texture: &scene.textures[*tex],
            material,
            model: *model,
        })
        .collect();
    let mut encoder = device.create_command_encoder(&Default::default());
    let lighting = Lighting::default();
    scene.renderer.render_view(
        device,
        &gpu.queue,
        &mut encoder,
        &mut view,
        camera,
        &lighting,
        style,
        &items,
    );
    let mut slots = vec![CompositeSlot {
        view: &view,
        rect,
        scissor: if split {
            rect.horizontal_slice(0.0, 0.5)
        } else {
            rect
        },
        style,
    }];
    if split {
        scene.renderer.render_view(
            device,
            &gpu.queue,
            &mut encoder,
            &mut reference,
            camera,
            &lighting,
            &reference_style,
            &items,
        );
        slots.push(CompositeSlot {
            view: &reference,
            rect,
            scissor: rect.horizontal_slice(0.5, 1.0),
            style: &reference_style,
        });
    }
    let target_view = target.create_view(&Default::default());
    scene
        .renderer
        .composite(&gpu.queue, &mut encoder, &target_view, output, &slots);

    let bytes_per_row = (output.0 * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row * output.1),
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
                rows_per_image: Some(output.1),
            },
        },
        wgpu::Extent3d {
            width: output.0,
            height: output.1,
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
    for row in 0..output.1 {
        let start = (row * bytes_per_row) as usize;
        rgba.extend_from_slice(&mapped[start..start + (output.0 * 4) as usize]);
    }
    rgba
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
fn camera_sweep_produces_no_gpu_errors() {
    let Some(gpu) = gpu_or_skip() else { return };
    let scene = upload(&gpu);
    let mut coarse = RetroStyle::ps1();
    coarse.vertex_snap.grid = 8.0;
    let styles = [RetroStyle::ps1(), coarse];

    let positions = [
        comparison_scene::START_POSITION,
        Vec3::new(0.0, 1.6, -20.0),  // in the corridor
        Vec3::new(-6.0, 1.5, 0.0),   // inside the left wall plane
        Vec3::new(1.5, 1.0, -12.0),  // inside the corridor wall
        Vec3::new(0.0, 0.001, 0.0),  // grazing the floor
        Vec3::new(0.0, -0.5, 0.0),   // below the floor
        Vec3::new(0.0, 1.3, -2.0),   // inside the crate
        Vec3::new(30.0, 10.0, 30.0), // outside the level
    ];
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut frames = 0;
    for style in &styles {
        for &position in &positions {
            for yaw_step in 0..4 {
                for pitch in [-1.55, 0.0, 1.55] {
                    let camera = Camera {
                        position,
                        yaw: yaw_step as f32 * std::f32::consts::FRAC_PI_2 + 0.3,
                        pitch,
                        ..Camera::default()
                    };
                    render(&gpu, &scene, style, &camera, (320, 240), false);
                    frames += 1;
                }
            }
        }
    }
    if let Some(err) = pollster::block_on(scope.pop()) {
        panic!("wgpu validation error during camera sweep: {err}");
    }
    assert_eq!(frames, 2 * 8 * 4 * 3);
}

#[test]
fn comparison_captures() {
    let Some(gpu) = gpu_or_skip() else { return };
    let scene = upload(&gpu);
    let camera = Camera {
        position: comparison_scene::START_POSITION,
        pitch: comparison_scene::START_PITCH,
        ..Camera::default()
    };
    let output = (640, 480);
    let ps1 = render(&gpu, &scene, &RetroStyle::ps1(), &camera, output, false);
    let clean = render(&gpu, &scene, &RetroStyle::clean(), &camera, output, false);
    let split = render(&gpu, &scene, &RetroStyle::ps1(), &camera, output, true);
    save_ppm("comparison_ps1.ppm", output, &ps1);
    save_ppm("comparison_clean.ppm", output, &clean);
    save_ppm("comparison_split.ppm", output, &split);
    assert_ne!(ps1, clean, "PS1 and clean renders must differ");
    // Split view: left half matches the PS1 render, right half the clean
    // render of the same rectangle (no letterbox at 4:3).
    for y in [10, 240, 470] {
        let row = |img: &Vec<u8>, x: u32| img[((y * output.0 + x) * 4) as usize..][..4].to_vec();
        assert_eq!(row(&split, 100), row(&ps1, 100), "left half, row {y}");
        assert_eq!(row(&split, 540), row(&clean, 540), "right half, row {y}");
    }
}
