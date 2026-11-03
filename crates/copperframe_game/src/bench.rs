//! Performance measurement: `<game> --bench` plays a scene with the real
//! simulation and renders every frame offscreen (no window), once per
//! style (as authored, PS1, PS2), and prints a JSON report of simulation
//! tick and frame times.
//!
//! A frame's time is measured from the start of the tick to the GPU
//! finishing the frame (the CPU waits for the GPU each frame), so it is an
//! upper bound on the frame cost without pipelining. Software adapters
//! (lavapipe, WARP) report much slower times than hardware GPUs; the
//! report names the adapter so numbers are only compared like for like.

use std::path::Path;
use std::time::Instant;

use copperframe_assets::project::Project;
use copperframe_core::RetroStyle;
use copperframe_render::{
    Camera, CompositeSlot, GpuContext, Lighting, RetroRenderer, ViewLayout, wgpu,
};
use copperframe_runtime::scene_gpu::{SceneResources, scene_lighting};
use serde_json::{Value, json};

use crate::actions::ActionState;
use crate::api::Game;
use crate::audio::Audio;
use crate::headless::Script;
use crate::sim::Simulation;

pub const BENCH_FORMAT: &str = "psx_bench";
pub const BENCH_VERSION: u32 = 1;
/// Frames rendered before measuring each style (pipeline warm-up, uploads).
pub const WARMUP_FRAMES: u32 = 10;
pub const MAX_BENCH_FRAMES: u32 = 100_000;

#[derive(Debug, Clone)]
pub struct BenchOptions {
    /// Measured frames per style.
    pub frames: u32,
    /// Output (window-equivalent) size in pixels.
    pub size: (u32, u32),
    /// Input for the simulation; idle when `None`. The script restarts for
    /// each style so every style sees the same frames.
    pub script: Option<Script>,
}

/// Mean, median, 95th percentile and maximum in milliseconds.
pub fn stats(samples: &[f64]) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    let mut v: Vec<f64> = samples.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let r = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "mean_ms": r(mean),
        "p50_ms": r(pick(0.5)),
        "p95_ms": r(pick(0.95)),
        "max_ms": r(v[v.len() - 1]),
    })
}

/// Offscreen colour target and readback-free submission.
struct Target {
    texture: wgpu::Texture,
    size: (u32, u32),
}

/// Runs the benchmark. Errors are reported as text (no adapter, scene
/// cannot be played).
pub fn run(
    project: Project,
    scene: &Path,
    create: fn() -> Box<dyn Game>,
    options: &BenchOptions,
) -> Result<Value, String> {
    let gpu = pollster::block_on(GpuContext::headless())
        .map_err(|e| format!("no graphics adapter for the benchmark: {e}"))?;
    let info = gpu.adapter.get_info();
    let device = &gpu.device;
    let queue = &gpu.queue;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let renderer = RetroRenderer::new(device, format);
    let mut resources = SceneResources::new(&renderer, device, queue);
    let size = (
        options.size.0.clamp(16, 8192),
        options.size.1.clamp(16, 8192),
    );
    let target = Target {
        texture: device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        }),
        size,
    };
    let frames = options.frames.clamp(1, MAX_BENCH_FRAMES);
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let mut results = Vec::new();
    let mut title = String::new();
    let mut scene_name = String::new();
    let mut warnings = Vec::new();
    for style_name in ["scene", "ps1", "ps2"] {
        // A fresh simulation per style so each measures the same frames.
        let mut sim = Simulation::new(project.clone(), create(), Audio::silent());
        sim.load_scene_file(scene).map_err(|e| e.to_string())?;
        title = sim.title();
        warnings.extend(sim.take_warnings());
        let Some(authored) = sim.scene().map(|s| s.settings.style.clone()) else {
            return Err("the scene did not load".into());
        };
        scene_name = sim.scene().map(|s| s.name.clone()).unwrap_or_default();
        let style = match style_name {
            "ps1" => crate::settings::GraphicsStyle::Ps1.apply(&authored),
            "ps2" => crate::settings::GraphicsStyle::Ps2.apply(&authored),
            _ => authored,
        };
        let layout = ViewLayout::for_style(size, &style)
            .ok_or_else(|| format!("style {style_name} has no layout at {size:?}"))?;
        let mut view = renderer
            .create_view(device, layout.internal.0, layout.internal.1)
            .map_err(|e| e.to_string())?;
        let inputs = script_inputs(options.script.as_ref());
        let mut tick_ms = Vec::with_capacity(frames as usize);
        let mut frame_ms = Vec::with_capacity(frames as usize);
        let mut triangles = 0u64;
        let mut draws_count = 0usize;
        for i in 0..(WARMUP_FRAMES + frames) {
            let input = inputs
                .get(i as usize)
                .cloned()
                .unwrap_or_else(|| ActionState::scripted(&[], &[], glam::Vec2::ZERO));
            let start = Instant::now();
            sim.tick(&input);
            let ticked = Instant::now();
            let stats = render_frame(
                &gpu,
                &renderer,
                &mut resources,
                &sim,
                &style,
                &mut view,
                &layout,
                &target,
            );
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| format!("GPU wait failed: {e}"))?;
            let end = Instant::now();
            if i >= WARMUP_FRAMES {
                tick_ms.push((ticked - start).as_secs_f64() * 1000.0);
                frame_ms.push((end - start).as_secs_f64() * 1000.0);
                (draws_count, triangles) = stats;
            }
        }
        warnings.extend(sim.take_warnings());
        sim.shutdown();
        results.push(json!({
            "style": style_name,
            "internal": [layout.internal.0, layout.internal.1],
            "draws": draws_count,
            "triangles": triangles,
            "tick": stats(&tick_ms),
            "frame": stats(&frame_ms),
            "fps_from_mean": stats(&frame_ms)["mean_ms"]
                .as_f64()
                .filter(|m| *m > 0.0)
                .map(|m| (1000.0 / m * 10.0).round() / 10.0),
        }));
    }
    if let Some(err) = pollster::block_on(scope.pop()) {
        return Err(format!("GPU validation error during the benchmark: {err}"));
    }
    warnings.sort();
    warnings.dedup();
    Ok(json!({
        "format": BENCH_FORMAT,
        "version": BENCH_VERSION,
        "game": title,
        "scene": scene_name,
        "adapter": {
            "name": info.name,
            "backend": format!("{:?}", info.backend),
            "device_type": format!("{:?}", info.device_type),
            "driver": info.driver,
            "driver_info": info.driver_info,
        },
        "output": [size.0, size.1],
        "frames_per_style": frames,
        "warmup_frames": WARMUP_FRAMES,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "styles": results,
        "warnings": warnings,
    }))
}

/// Per-tick input expanded from a script (first tick of a step gets its
/// presses).
fn script_inputs(script: Option<&Script>) -> Vec<ActionState> {
    let mut out = Vec::new();
    for st in script.map(|s| s.steps.as_slice()).unwrap_or_default() {
        let hold: Vec<&str> = st.hold.iter().map(String::as_str).collect();
        let press: Vec<&str> = st.press.iter().map(String::as_str).collect();
        let look = glam::Vec2::from(st.look);
        for i in 0..st.ticks.min(u64::from(MAX_BENCH_FRAMES)) {
            out.push(ActionState::scripted(
                &hold,
                if i == 0 { &press } else { &[] },
                look,
            ));
        }
    }
    out
}

/// Renders the simulation's current state; returns (draws, triangles).
#[allow(clippy::too_many_arguments)]
fn render_frame(
    gpu: &GpuContext,
    renderer: &RetroRenderer,
    resources: &mut SceneResources,
    sim: &Simulation,
    style: &RetroStyle,
    view: &mut copperframe_render::RenderView,
    layout: &ViewLayout,
    target: &Target,
) -> (usize, u64) {
    let device = &gpu.device;
    let draws = sim.draw_list();
    let messages = resources.sync_meshes(
        sim.project(),
        sim.asset_db(),
        draws.iter().map(|d| &d.mesh),
        renderer,
        device,
        &gpu.queue,
    );
    for m in messages {
        tracing::warn!("{m}");
    }
    let lighting = match sim.scene() {
        Some(s) => {
            let lights = sim.lights(1.0);
            let shadows = sim.shadows(1.0);
            scene_lighting(
                &s.settings.lighting,
                lights.iter().map(|(m, l)| (*m, l)),
                shadows.iter().map(|(m, s)| (*m, s)),
            )
        }
        None => Lighting::default(),
    };
    let cam = sim.camera(1.0);
    let camera = Camera {
        position: cam.position,
        yaw: cam.yaw,
        pitch: cam.pitch,
        fov_y: cam.fov_deg.to_radians(),
        ..Camera::default()
    };
    let items = draws
        .iter()
        .map(|d| (d.id, &d.mesh, d.current, d.joints.as_deref()));
    let scene_draws = resources.draws_from(items);
    let triangles = scene_draws.iter().map(|d| u64::from(d.triangles)).sum();
    let count = scene_draws.len();
    let items: Vec<_> = scene_draws.iter().map(|d| d.item).collect();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench frame"),
    });
    renderer.render_view(
        device,
        &gpu.queue,
        &mut encoder,
        view,
        &camera,
        &lighting,
        style,
        &items,
    );
    let output = target.texture.create_view(&Default::default());
    let rect = layout.placement.rect;
    renderer.composite(
        &gpu.queue,
        &mut encoder,
        &output,
        target.size,
        &[CompositeSlot {
            view,
            rect,
            scissor: rect,
            style,
        }],
    );
    gpu.queue.submit([encoder.finish()]);
    (count, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_ordered_and_ignore_non_finite_samples() {
        let s = stats(&[4.0, 1.0, 3.0, f64::NAN, 2.0, 100.0]);
        assert_eq!(s["p50_ms"], 3.0);
        assert_eq!(s["max_ms"], 100.0);
        assert_eq!(s["mean_ms"], 22.0);
        assert!(stats(&[]).is_null());
    }
}
