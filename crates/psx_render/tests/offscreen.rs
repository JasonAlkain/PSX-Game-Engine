//! Renders the Milestone 1 test mesh offscreen and checks the result.
//!
//! Needs a graphics adapter (a software one such as Mesa lavapipe is fine).
//! Without one the test prints `SKIPPED` and returns, unless
//! `PSX_REQUIRE_GPU=1` is set, in which case a missing adapter fails the test.
//! A skipped run is not a passed render check.

use glam::{Mat4, Vec3};
use psx_render::{
    Camera, DepthTarget, FrameUniforms, GpuContext, Lighting, MeshData, MeshRenderer, TextureData,
    wgpu,
};

const SIZE: u32 = 64; // 64 px * 4 bytes = 256, already COPY_BYTES_PER_ROW_ALIGNMENT.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu_or_skip() -> Option<GpuContext> {
    match pollster::block_on(GpuContext::headless()) {
        Ok(gpu) => Some(gpu),
        Err(err) => {
            if std::env::var("PSX_REQUIRE_GPU").as_deref() == Ok("1") {
                panic!("PSX_REQUIRE_GPU=1 but no adapter is available: {err}");
            }
            eprintln!("SKIPPED: no graphics adapter available ({err})");
            None
        }
    }
}

fn render_cube(gpu: &GpuContext, clear: wgpu::Color) -> Vec<u8> {
    let device = &gpu.device;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let renderer = MeshRenderer::new(device, FORMAT);
    let mesh = renderer
        .upload_mesh(device, &MeshData::cube(1.0))
        .expect("cube uploads");
    let texture = renderer
        .upload_texture(
            device,
            &gpu.queue,
            &TextureData::checkerboard(64, 8, [235, 235, 235, 255], [60, 60, 60, 255]),
        )
        .expect("checkerboard uploads");
    let depth = DepthTarget::new(device, SIZE, SIZE);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen color"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let camera = Camera {
        position: Vec3::new(0.0, 0.0, 2.5),
        ..Camera::default()
    };
    let model = Mat4::from_rotation_y(0.6) * Mat4::from_rotation_x(0.4);
    let uniforms = FrameUniforms::new(camera.view_projection(1.0), model, &Lighting::default());

    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw(
        &gpu.queue,
        &mut encoder,
        &view,
        &depth,
        clear,
        &uniforms,
        &mesh,
        &texture,
    );
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
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);

    readback.slice(..).map_async(wgpu::MapMode::Read, |r| {
        r.expect("readback buffer maps");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");

    if let Some(err) = pollster::block_on(scope.pop()) {
        panic!("wgpu validation error during offscreen render: {err}");
    }

    readback
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .to_vec()
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

fn write_ppm(name: &str, pixels: &[u8]) {
    // Best-effort debug output under target/; failures do not fail the test.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let mut out = format!("P6\n{SIZE} {SIZE}\n255\n").into_bytes();
    for px in pixels.chunks(4) {
        out.extend_from_slice(&px[..3]);
    }
    let path = dir.join(name);
    if std::fs::write(&path, out).is_ok() {
        eprintln!("wrote {}", path.display());
    }
}

#[test]
fn renders_textured_cube_without_validation_errors() {
    let Some(gpu) = gpu_or_skip() else { return };

    let clear = wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    };
    let pixels = render_cube(&gpu, clear);
    write_ppm("offscreen_cube.ppm", &pixels);

    // Corners show the clear colour.
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1)] {
        assert_eq!(pixel(&pixels, x, y), [0, 0, 255, 255], "corner ({x},{y})");
    }

    // The cube covers the centre: not the clear colour, and opaque.
    let centre = pixel(&pixels, SIZE / 2, SIZE / 2);
    assert_ne!(centre, [0, 0, 255, 255], "cube missing at centre");

    // The texture is visible: covered pixels include both light and dark
    // checker cells, not a flat fill.
    let covered: Vec<[u8; 4]> = (0..SIZE * SIZE)
        .map(|i| pixel(&pixels, i % SIZE, i / SIZE))
        .filter(|p| *p != [0, 0, 255, 255])
        .collect();
    assert!(covered.len() > (SIZE * SIZE / 8) as usize, "cube too small");
    let luma = |p: &[u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let max = covered.iter().map(luma).max().unwrap();
    let min = covered.iter().map(luma).min().unwrap();
    assert!(
        max - min > 150,
        "expected checkerboard contrast, got {min}..{max}"
    );
}
