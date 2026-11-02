//! Window and event-loop lifecycle.
//!
//! Per frame (`RedrawRequested`):
//! 1. measure elapsed time and ask [`FixedStep`] how many ticks to run;
//! 2. for each tick: sample input, update the simulation (fly camera, test mesh);
//! 3. interpolate between the last two simulation states and render.
//!
//! Minimized or zero-sized windows skip rendering and stop requesting redraws
//! until they are restored. Focus loss releases mouse capture and held keys.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use glam::{Mat4, Vec2, Vec3};
use psx_core::FixedStep;
use psx_render::{
    Camera, DepthTarget, FrameAcquire, FrameUniforms, GpuContext, GpuMesh, GpuTexture, Lighting,
    MeshData, MeshRenderer, Presenter, TextureData, wgpu,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::fly_camera::FlyCamera;
use crate::input::InputState;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub title: String,
    /// Initial window size in logical (DPI-independent) pixels.
    pub width: u32,
    pub height: u32,
    /// Exit after presenting this many frames. Used for automated smoke runs.
    pub exit_after_frames: Option<u64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "PSX Engine".to_owned(),
            width: 960,
            height: 720,
            exit_after_frames: None,
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

/// Linear clear colour (dusky blue-grey, displayed after sRGB encoding).
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.025,
    b: 0.05,
    a: 1.0,
};

/// Consecutive surface recreation failures tolerated before giving up.
const MAX_SURFACE_FAILURES: u32 = 5;

/// Simulation state that is interpolated for rendering.
#[derive(Debug, Clone, Copy)]
struct SimState {
    camera: Camera,
    mesh_angle: f32,
}

impl SimState {
    fn lerp(&self, next: &SimState, t: f32) -> SimState {
        SimState {
            camera: self.camera.lerp(&next.camera, t),
            mesh_angle: self.mesh_angle + (next.mesh_angle - self.mesh_angle) * t,
        }
    }
}

struct App {
    display: OwnedDisplayHandle,
    config: AppConfig,
    running: Option<Running>,
    error: Option<anyhow::Error>,
}

struct Running {
    window: Arc<Window>,
    gpu: GpuContext,
    presenter: Presenter,
    depth: DepthTarget,
    renderer: MeshRenderer,
    mesh: GpuMesh,
    texture: GpuTexture,
    lighting: Lighting,

    stepper: FixedStep,
    previous: SimState,
    current: SimState,
    fly: FlyCamera,
    input: InputState,
    last_frame: Instant,

    minimized: bool,
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
        let depth = DepthTarget::new(&gpu.device, size.width, size.height);
        let renderer = MeshRenderer::new(&gpu.device, presenter.format());
        let mesh = renderer
            .upload_mesh(&gpu.device, &MeshData::cube(1.0))
            .context("test mesh is invalid")?;
        let texture = renderer
            .upload_texture(
                &gpu.device,
                &gpu.queue,
                &TextureData::checkerboard(64, 8, [225, 220, 205, 255], [70, 60, 80, 255]),
            )
            .context("test texture is invalid")?;

        let initial = SimState {
            camera: Camera {
                position: Vec3::new(0.0, 0.6, 3.0),
                pitch: -0.15,
                ..Camera::default()
            },
            mesh_angle: 0.0,
        };

        tracing::info!(
            width = size.width,
            height = size.height,
            scale_factor = window.scale_factor(),
            "window ready; click to capture the mouse, WASD/QE to fly, Shift for speed, Esc to release"
        );

        Ok(Self {
            window,
            gpu,
            presenter,
            depth,
            renderer,
            mesh,
            texture,
            lighting: Lighting::default(),
            stepper: FixedStep::default(),
            previous: initial,
            current: initial,
            fly: FlyCamera::default(),
            input: InputState::default(),
            last_frame: Instant::now(),
            minimized: size.width == 0 || size.height == 0,
            surface_failures: 0,
            fatal: None,
            cursor_captured: false,
            frames_presented: 0,
            stats_window_start: Instant::now(),
            stats_frames: 0,
        })
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
            self.depth.ensure_size(&self.gpu.device, width, height);
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
            self.current.mesh_angle =
                (self.current.mesh_angle + dt * 0.8).rem_euclid(std::f32::consts::TAU * 4.0);
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
            FrameAcquire::Skip => return false,
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
        let (w, h) = self.presenter.size();
        let aspect = w as f32 / h as f32;
        let model = Mat4::from_rotation_y(state.mesh_angle)
            * Mat4::from_rotation_x(state.mesh_angle * 0.37);
        let uniforms =
            FrameUniforms::new(state.camera.view_projection(aspect), model, &self.lighting);

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.renderer.draw(
            &self.gpu.queue,
            &mut encoder,
            &view,
            &self.depth,
            CLEAR_COLOR,
            &uniforms,
            &self.mesh,
            &self.texture,
        );
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
                if !occluded {
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
        let Some(app) = self.running.as_ref() else {
            return;
        };
        if app.minimized {
            // Sleep until the OS sends a restore/resize event.
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
            app.window.request_redraw();
        }
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
