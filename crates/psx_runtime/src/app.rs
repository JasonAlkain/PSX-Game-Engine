//! Window and event-loop lifecycle.
//!
//! Per frame (`RedrawRequested`):
//! 1. measure elapsed time and ask [`FixedStep`] how many ticks to run;
//! 2. for each tick: sample input, update the simulation (fly camera, crate spin);
//! 3. interpolate between the last two simulation states and render the
//!    comparison scene through the retro renderer (scene pass at internal
//!    resolution, then composite into the window).
//!
//! Minimized or zero-sized windows skip rendering and stop requesting redraws
//! until they are restored. Focus loss releases mouse capture and held keys.
//! Style keys (see [`crate::style_controls`]) change the style immediately;
//! they are UI actions, not simulation input.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use glam::{Mat4, Vec2};
use psx_core::style::{StyleError, load_style, save_style};
use psx_core::{FixedStep, RetroStyle};
use psx_render::{
    Camera, CompositeSlot, DrawItem, FrameAcquire, GpuContext, GpuMesh, GpuTexture, Lighting,
    Material, Presenter, RenderView, RetroRenderer, ViewLayout, wgpu,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::comparison_scene;
use crate::fly_camera::FlyCamera;
use crate::input::InputState;
use crate::style_controls::{self, StyleCommand};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub title: String,
    /// Initial window size in logical (DPI-independent) pixels.
    pub width: u32,
    pub height: u32,
    /// Exit after presenting this many frames. Used for automated smoke runs.
    pub exit_after_frames: Option<u64>,
    /// Style settings file: loaded at start if present, written by F5.
    pub style_path: PathBuf,
    /// Start in split comparison view.
    pub split: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "PSX Engine".to_owned(),
            width: 960,
            height: 720,
            exit_after_frames: None,
            style_path: PathBuf::from("psx_style.json"),
            split: false,
        }
    }
}

/// Opens a window and runs until it is closed (or `exit_after_frames` is reached).
pub fn run(config: AppConfig) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("could not create the window event loop")?;
    let mut app = App {
        display: event_loop.owned_display_handle(),
        config,
        running: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("window event loop failed")?;
    match app.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Consecutive surface recreation failures tolerated before giving up.
const MAX_SURFACE_FAILURES: u32 = 5;

/// Simulation state that is interpolated for rendering.
#[derive(Debug, Clone, Copy)]
struct SimState {
    camera: Camera,
    /// Crate rotation about Y and X, each kept in `[0, TAU)`.
    spin_y: f32,
    spin_x: f32,
}

impl SimState {
    fn lerp(&self, next: &SimState, t: f32) -> SimState {
        SimState {
            camera: self.camera.lerp(&next.camera, t),
            spin_y: lerp_angle(self.spin_y, next.spin_y, t),
            spin_x: lerp_angle(self.spin_x, next.spin_x, t),
        }
    }
}

/// Interpolates between two angles (radians) along the shorter arc, so a
/// value that wrapped from just below TAU to just above 0 does not spin back.
fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut delta = (to - from).rem_euclid(TAU);
    if delta > PI {
        delta -= TAU;
    }
    from + delta * t
}

/// Rotation speeds of the spinning crate in radians per second.
const SPIN_Y_SPEED: f32 = 0.8;
const SPIN_X_SPEED: f32 = 0.8 * 0.37;

/// How long to wait before retrying after a frame could not be acquired
/// (occluded, timed out, reconfigured), instead of spinning the CPU.
const ACQUIRE_RETRY: Duration = Duration::from_millis(50);

struct App {
    display: OwnedDisplayHandle,
    config: AppConfig,
    running: Option<Running>,
    error: Option<anyhow::Error>,
}

/// One uploaded object of the comparison scene.
struct GpuObject {
    mesh: GpuMesh,
    texture: usize,
    material: Material,
    transform: Mat4,
    spins: bool,
}

struct Running {
    window: Arc<Window>,
    gpu: GpuContext,
    presenter: Presenter,
    renderer: RetroRenderer,
    /// The view rendered with the current style.
    view: RenderView,
    /// Effects-off view for the split comparison; created on first use.
    reference_view: Option<RenderView>,
    textures: Vec<GpuTexture>,
    objects: Vec<GpuObject>,
    lighting: Lighting,
    style: RetroStyle,
    style_path: PathBuf,
    split: bool,
    base_title: String,

    stepper: FixedStep,
    previous: SimState,
    current: SimState,
    fly: FlyCamera,
    input: InputState,
    last_frame: Instant,

    minimized: bool,
    /// Set from `WindowEvent::Occluded`; rendering pauses while true.
    occluded: bool,
    /// After a skipped acquire, the next redraw waits until this instant.
    retry_at: Option<Instant>,
    surface_failures: u32,
    /// Unrecoverable error that ended the event loop.
    fatal: Option<anyhow::Error>,
    cursor_captured: bool,
    frames_presented: u64,
    stats_window_start: Instant,
    stats_frames: u32,
}

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        display: &OwnedDisplayHandle,
        config: &AppConfig,
    ) -> anyhow::Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(config.title.clone())
            .with_inner_size(LogicalSize::new(config.width, config.height))
            .with_min_inner_size(LogicalSize::new(160, 120));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the main window")?,
        );

        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display.clone())),
        );
        let surface = instance
            .create_surface(window.clone())
            .context("could not create a rendering surface for the window")?;
        let gpu = pollster::block_on(GpuContext::new(instance, Some(&surface)))
            .context("graphics initialisation failed; check that GPU drivers are installed")?;

        let size = window.inner_size();
        let presenter = Presenter::new(&gpu, surface, size.width, size.height)?;
        let renderer = RetroRenderer::new(&gpu.device, presenter.format());

        let style = initial_style(&config.style_path);
        check_device_limits(&gpu, &style)?;
        let view = renderer
            .create_view(&gpu.device, style.low_res.width, style.low_res.height)
            .context("could not create the internal render target")?;

        let scene = comparison_scene::build();
        let textures = scene
            .textures
            .iter()
            .enumerate()
            .map(|(i, t)| {
                renderer
                    .upload_texture(&gpu.device, &gpu.queue, t)
                    .with_context(|| format!("comparison scene texture {i} is invalid"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let objects = scene
            .objects
            .into_iter()
            .map(|o| {
                Ok(GpuObject {
                    mesh: renderer
                        .upload_mesh(&gpu.device, &o.mesh)
                        .with_context(|| {
                            format!("comparison scene mesh `{}` is invalid", o.name)
                        })?,
                    texture: o.texture,
                    material: o.material,
                    transform: o.transform,
                    spins: o.spins,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let initial = SimState {
            camera: Camera {
                position: comparison_scene::START_POSITION,
                pitch: comparison_scene::START_PITCH,
                ..Camera::default()
            },
            spin_y: 0.0,
            spin_x: 0.0,
        };

        tracing::info!(
            width = size.width,
            height = size.height,
            scale_factor = window.scale_factor(),
            "window ready; click to capture the mouse, WASD/QE to fly, Shift for speed, Esc to release"
        );
        tracing::info!("{}", style_controls::help_text());

        let running = Self {
            window,
            gpu,
            presenter,
            renderer,
            view,
            reference_view: None,
            textures,
            objects,
            lighting: crate::comparison_scene::lighting(),
            style,
            style_path: config.style_path.clone(),
            split: config.split,
            base_title: config.title.clone(),
            stepper: FixedStep::default(),
            previous: initial,
            current: initial,
            fly: FlyCamera::default(),
            input: InputState::default(),
            last_frame: Instant::now(),
            minimized: size.width == 0 || size.height == 0,
            occluded: false,
            retry_at: None,
            surface_failures: 0,
            fatal: None,
            cursor_captured: false,
            frames_presented: 0,
            stats_window_start: Instant::now(),
            stats_frames: 0,
        };
        running.update_title();
        Ok(running)
    }

    fn update_title(&self) {
        let split = if self.split {
            "  [split: left current, right effects off]"
        } else {
            ""
        };
        self.window.set_title(&format!(
            "{} — {}{split}",
            self.base_title,
            style_controls::summary(&self.style)
        ));
    }

    fn handle_style_key(&mut self, key: KeyCode) {
        let Some(command) = style_controls::apply_key(&mut self.style, key) else {
            return;
        };
        match command {
            StyleCommand::Changed(what) => tracing::info!("{what}"),
            StyleCommand::ToggleSplit => {
                self.split = !self.split;
                tracing::info!(split = self.split, "comparison view");
            }
            StyleCommand::Save => match save_style(&self.style, &self.style_path) {
                Ok(()) => tracing::info!(path = %self.style_path.display(), "style saved"),
                Err(err) => tracing::error!("{err}"),
            },
            StyleCommand::Reload => {
                let loaded = load_style(&self.style_path).map_err(anyhow::Error::from);
                match loaded.and_then(|s| check_device_limits(&self.gpu, &s).map(|()| s)) {
                    Ok(style) => {
                        self.style = style;
                        tracing::info!(path = %self.style_path.display(), "style reloaded");
                    }
                    Err(err) => tracing::error!("style not reloaded: {err:#}"),
                }
            }
            StyleCommand::Help => tracing::info!("{}", style_controls::help_text()),
        }
        self.update_title();
    }

    fn set_cursor_capture(&mut self, capture: bool) {
        if capture == self.cursor_captured {
            return;
        }
        if capture {
            let grabbed = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(err) = grabbed {
                tracing::warn!("mouse capture unavailable: {err}");
                return;
            }
            self.window.set_cursor_visible(false);
        } else {
            if let Err(err) = self.window.set_cursor_grab(CursorGrabMode::None) {
                tracing::warn!("could not release mouse capture: {err}");
            }
            self.window.set_cursor_visible(true);
        }
        self.cursor_captured = capture;
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.minimized = width == 0 || height == 0;
        self.presenter.resize(&self.gpu, width, height);
        if !self.minimized {
            // Internal targets follow the style's layout in `redraw`.
            tracing::debug!(width, height, "resized");
        } else {
            tracing::debug!("minimized; rendering paused");
        }
    }

    fn simulate(&mut self, frame_time: Duration) -> f32 {
        let plan = self.stepper.advance(frame_time);
        if !plan.dropped.is_zero() {
            tracing::debug!(
                dropped_ms = plan.dropped.as_millis(),
                "simulation fell behind"
            );
        }
        let dt = self.stepper.step_secs();
        for _ in 0..plan.ticks {
            self.previous = self.current;
            let tick = self.input.begin_tick();
            self.fly
                .tick(&mut self.current.camera, &tick, dt, self.cursor_captured);
            let tau = std::f32::consts::TAU;
            self.current.spin_y = (self.current.spin_y + dt * SPIN_Y_SPEED).rem_euclid(tau);
            self.current.spin_x = (self.current.spin_x + dt * SPIN_X_SPEED).rem_euclid(tau);
        }
        plan.alpha
    }

    /// Returns `true` when a frame was presented.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let now = Instant::now();
        let frame_time = now - self.last_frame;
        self.last_frame = now;
        let alpha = self.simulate(frame_time);

        if self.minimized {
            return false;
        }
        let frame = match self.presenter.acquire(&self.gpu) {
            FrameAcquire::Ready(frame) => frame,
            FrameAcquire::Skip => {
                self.retry_at = Some(now + ACQUIRE_RETRY);
                return false;
            }
            FrameAcquire::Lost => {
                tracing::warn!("rendering surface lost; recreating");
                let (instance, window) = (&self.gpu.instance, self.window.clone());
                let recreated = self
                    .presenter
                    .recreate_surface(&self.gpu, || instance.create_surface(window));
                if let Err(err) = recreated {
                    self.surface_failures += 1;
                    tracing::error!(
                        attempt = self.surface_failures,
                        "could not recreate the rendering surface: {err}"
                    );
                    if self.surface_failures >= MAX_SURFACE_FAILURES {
                        self.fatal = Some(anyhow::anyhow!(
                            "rendering surface could not be recreated after {} attempts: {err}",
                            self.surface_failures
                        ));
                        event_loop.exit();
                    }
                } else {
                    self.surface_failures = 0;
                }
                return false;
            }
        };

        let state = self.previous.lerp(&self.current, alpha);
        let output_size = self.presenter.size();
        let Some(layout) = ViewLayout::for_style(output_size, &self.style) else {
            return false;
        };
        let reference_style = style_controls::reference_style(&self.style);
        let device = &self.gpu.device;
        let (iw, ih) = layout.internal;
        if let Err(err) = self
            .renderer
            .ensure_view_size(device, &mut self.view, iw, ih)
        {
            tracing::error!("{err}");
            return false;
        }
        let rect = layout.placement.rect;
        if self.split {
            // The reference renders at the native size of the same rectangle.
            let result = match &mut self.reference_view {
                Some(view) => self
                    .renderer
                    .ensure_view_size(device, view, rect.width, rect.height),
                None => self
                    .renderer
                    .create_view(device, rect.width, rect.height)
                    .map(|v| {
                        self.reference_view = Some(v);
                    }),
            };
            if let Err(err) = result {
                tracing::error!("{err}");
                self.split = false;
            }
        }

        let spin = Mat4::from_rotation_y(state.spin_y) * Mat4::from_rotation_x(state.spin_x);
        let items: Vec<DrawItem<'_>> = self
            .objects
            .iter()
            .map(|o| DrawItem {
                mesh: &o.mesh,
                texture: &self.textures[o.texture],
                material: &o.material,
                model: if o.spins {
                    o.transform * spin
                } else {
                    o.transform
                },
                joints: None,
            })
            .collect();

        let output_view = frame.texture.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        let queue = &self.gpu.queue;
        self.renderer.render_view(
            device,
            queue,
            &mut encoder,
            &mut self.view,
            &state.camera,
            &self.lighting,
            &self.style,
            &items,
        );
        let reference = match (&mut self.reference_view, self.split) {
            (Some(view), true) => {
                self.renderer.render_view(
                    device,
                    queue,
                    &mut encoder,
                    view,
                    &state.camera,
                    &self.lighting,
                    &reference_style,
                    &items,
                );
                Some(&*view)
            }
            _ => None,
        };
        let mut slots = vec![CompositeSlot {
            view: &self.view,
            rect,
            scissor: if reference.is_some() {
                rect.horizontal_slice(0.0, 0.5)
            } else {
                rect
            },
            style: &self.style,
        }];
        if let Some(view) = reference {
            slots.push(CompositeSlot {
                view,
                rect,
                scissor: rect.horizontal_slice(0.5, 1.0),
                style: &reference_style,
            });
        }
        self.renderer
            .composite(queue, &mut encoder, &output_view, output_size, &slots);
        drop(items);
        self.gpu.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);

        self.frames_presented += 1;
        self.stats_frames += 1;
        let elapsed = self.stats_window_start.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let fps = f64::from(self.stats_frames) / elapsed.as_secs_f64();
            tracing::info!(
                fps = format_args!("{fps:.1}"),
                ticks = self.stepper.total_ticks(),
                "frame stats"
            );
            self.stats_window_start = Instant::now();
            self.stats_frames = 0;
        }
        true
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match Running::new(event_loop, &self.display, &self.config) {
            Ok(running) => {
                running.window.request_redraw();
                self.running = Some(running);
            }
            Err(err) => {
                tracing::error!("{err:#}");
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(app) = self.running.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Destroyed => {
                // The native window is gone (e.g. destroyed by another
                // process); its surface cannot be recreated.
                tracing::warn!("window destroyed; exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => app.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // winit follows this with a Resized event carrying the new
                // physical size; nothing else depends on DPI yet.
                tracing::debug!(scale_factor, "DPI scale changed");
            }
            WindowEvent::Occluded(occluded) => {
                tracing::debug!(occluded, "window occlusion changed");
                app.occluded = occluded;
                if !occluded {
                    app.retry_at = None;
                    app.window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                app.set_cursor_capture(false);
                app.input.clear();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => app.set_cursor_capture(true),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match event.state {
                    ElementState::Pressed => {
                        if code == KeyCode::Escape {
                            app.set_cursor_capture(false);
                        }
                        if !event.repeat {
                            app.handle_style_key(code);
                        }
                        app.input.key_down(code, event.repeat);
                    }
                    ElementState::Released => app.input.key_up(code),
                }
            }
            WindowEvent::RedrawRequested if event_loop.exiting() => {}
            WindowEvent::RedrawRequested => {
                let presented = app.redraw(event_loop);
                let limit_reached = self
                    .config
                    .exit_after_frames
                    .is_some_and(|n| app.frames_presented >= n);
                if presented && limit_reached {
                    tracing::info!(
                        frames = app.frames_presented,
                        "frame limit reached; exiting"
                    );
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let (Some(app), DeviceEvent::MouseMotion { delta }) = (self.running.as_mut(), event)
            && app.cursor_captured
        {
            app.input
                .mouse_moved(Vec2::new(delta.0 as f32, delta.1 as f32));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(app) = self.running.as_mut() else {
            return;
        };
        if app.minimized || app.occluded {
            // Sleep until the OS sends a restore/resize/occlusion event.
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if let Some(retry_at) = app.retry_at {
            if Instant::now() < retry_at {
                event_loop.set_control_flow(ControlFlow::WaitUntil(retry_at));
                return;
            }
            app.retry_at = None;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        app.window.request_redraw();
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(app) = self.running.as_mut() {
            app.set_cursor_capture(false);
            tracing::info!(frames = app.frames_presented, "shutting down");
        }
        // Drop GPU resources before the window and event loop go away.
        if let Some(app) = self.running.take()
            && let Some(err) = app.fatal
        {
            self.error = Some(err);
        }
    }
}

/// Loads the style file if it exists. A missing file means "use the PS1
/// preset"; an unreadable or invalid file is reported and also falls back to
/// the preset, without overwriting the file.
fn initial_style(path: &std::path::Path) -> RetroStyle {
    match load_style(path) {
        Ok(style) => {
            tracing::info!(path = %path.display(), "style settings loaded");
            style
        }
        Err(StyleError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                path = %path.display(),
                "no style settings file; using the PS1 preset (F5 saves)"
            );
            RetroStyle::ps1()
        }
        Err(err) => {
            tracing::error!("{err}; using the PS1 preset. The file is left unchanged until F5.");
            RetroStyle::ps1()
        }
    }
}

/// Rejects internal resolutions the device cannot allocate.
fn check_device_limits(gpu: &GpuContext, style: &RetroStyle) -> anyhow::Result<()> {
    let max = gpu.device.limits().max_texture_dimension_2d;
    let (w, h) = (style.low_res.width, style.low_res.height);
    if w > max || h > max {
        anyhow::bail!("internal resolution {w}x{h} exceeds this GPU's limit of {max} pixels");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn angle_lerp_takes_short_arc_across_wrap() {
        let mid = lerp_angle(TAU - 0.1, 0.1, 0.5);
        assert!(mid.rem_euclid(TAU) < 1e-5 || (TAU - mid.rem_euclid(TAU)) < 1e-5);
        assert!((lerp_angle(1.0, 2.0, 0.25) - 1.25).abs() < 1e-6);
    }

    #[test]
    fn spin_interpolation_has_no_pop_when_angles_wrap() {
        let a = SimState {
            camera: Camera::default(),
            spin_y: TAU - 0.01,
            spin_x: TAU - 0.004,
        };
        let b = SimState {
            spin_y: 0.003,
            spin_x: 0.0003,
            ..a
        };
        for i in 0..=10 {
            let s = a.lerp(&b, i as f32 / 10.0);
            let ry = Mat4::from_rotation_y(s.spin_y);
            let ry_a = Mat4::from_rotation_y(a.spin_y);
            // Interpolated pose stays within the tiny step between the states.
            assert!(ry.abs_diff_eq(ry_a, 0.02), "pop at t={i}");
        }
    }
}
