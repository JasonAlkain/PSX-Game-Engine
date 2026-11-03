//! Editor window: winit event loop, egui integration and frame rendering.
//!
//! Per frame: run the egui UI (which may edit the scene through the
//! session), load newly referenced assets, render the scene into the
//! viewport rectangle through the retro renderer (scene pass at internal
//! resolution, composite letterboxed into the viewport), then draw egui on
//! top at native resolution so editor text stays legible.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use copperframe_render::{
    CompositeSlot, FrameAcquire, GpuContext, Presenter, RenderView, RetroRenderer, wgpu,
};
use copperframe_runtime::branding::Splash;
use copperframe_runtime::scene_gpu::{SceneResources, lighting_for_scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::window::{Window, WindowId};

use crate::gameplay::GameTools;
use crate::session::EditorSession;
use crate::ui::{self, Action, UiContext, UiState};
use crate::viewport::{EditorCamera, ViewportGeometry};

#[derive(Debug, Clone)]
pub struct EditorConfig {
    pub project: PathBuf,
    pub scene: Option<PathBuf>,
    /// Replace an editor lock left behind by a crash.
    pub steal_lock: bool,
    /// Exit after this many frames (automated smoke runs).
    pub exit_after_frames: Option<u64>,
}

pub fn run(config: EditorConfig) -> anyhow::Result<()> {
    let session = EditorSession::open(
        &config.project,
        config.scene.as_deref(),
        true,
        config.steal_lock,
    )
    .with_context(|| format!("could not open project {}", config.project.display()))?;
    let event_loop = EventLoop::new().context("could not create the window event loop")?;
    let mut app = App {
        display: event_loop.owned_display_handle(),
        config,
        session: Some(session),
        running: None,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("window event loop failed")?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct App {
    display: OwnedDisplayHandle,
    config: EditorConfig,
    /// Moved into `Running` once the window exists.
    session: Option<EditorSession>,
    running: Option<Running>,
    error: Option<anyhow::Error>,
}

struct Running {
    window: Arc<Window>,
    gpu: GpuContext,
    presenter: Presenter,
    renderer: RetroRenderer,
    view: RenderView,
    resources: SceneResources,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    session: EditorSession,
    tools: GameTools,
    ui: UiState,
    camera: EditorCamera,
    last_frame: Instant,
    /// Clock for the viewport's animation preview.
    started: Instant,
    minimized: bool,
    frames: u64,
    fps_window: (Instant, u32),
    quit: bool,
    splash: Splash,
}

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        display: &OwnedDisplayHandle,
        session: EditorSession,
    ) -> anyhow::Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Copperframe Engine editor")
                        .with_window_icon(copperframe_runtime::branding::window_icon())
                        .with_inner_size(LogicalSize::new(1440, 900))
                        .with_min_inner_size(LogicalSize::new(640, 400)),
                )
                .context("could not create the editor window")?,
        );
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display.clone())),
        );
        let surface = instance
            .create_surface(window.clone())
            .context("could not create a rendering surface")?;
        let gpu = pollster::block_on(GpuContext::new(instance, Some(&surface)))
            .context("graphics initialisation failed; check that GPU drivers are installed")?;
        let size = window.inner_size();
        let presenter = Presenter::new(&gpu, surface, size.width, size.height)?;
        let renderer = RetroRenderer::new(&gpu.device, presenter.format());
        let view = renderer
            .create_view(&gpu.device, 320, 240)
            .context("could not create the viewport render target")?;
        let resources = SceneResources::new(&renderer, &gpu.device, &gpu.queue);

        let egui_ctx = egui::Context::default();
        let max_texture = gpu.device.limits().max_texture_dimension_2d as usize;
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_texture),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            presenter.overlay_format(),
            egui_wgpu::RendererOptions::default(),
        );
        let camera = EditorCamera::from_start(&session.scene.settings.camera);
        let tools = GameTools::new(&session);
        let mut ui = UiState::default();
        ui.reporting.check_for_crashes();
        let running = Self {
            window,
            gpu,
            presenter,
            renderer,
            view,
            resources,
            egui_ctx,
            egui_state,
            egui_renderer,
            session,
            tools,
            ui,
            camera,
            last_frame: Instant::now(),
            started: Instant::now(),
            minimized: size.width == 0 || size.height == 0,
            frames: 0,
            fps_window: (Instant::now(), 0),
            quit: false,
            splash: Splash::new(),
        };
        running.update_title();
        Ok(running)
    }

    fn update_title(&self) {
        let dirty = if self.session.is_dirty() { "*" } else { "" };
        let title = format!(
            "{dirty}{} — {} — Copperframe Engine editor",
            self.session.project.relative(&self.session.scene_path),
            self.session.project.file.name
        );
        if self.window.title() != title {
            self.window.set_title(&title);
        }
    }

    fn sync_assets(&mut self) {
        let messages = self.resources.sync(
            &self.session.project,
            &self.session.db,
            &self.session.scene,
            &self.renderer,
            &self.gpu.device,
            &self.gpu.queue,
        );
        for m in messages {
            self.session.error(m);
        }
    }

    fn handle(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::Reimport(id) => {
                // Pick up edited sidecar settings too.
                self.session.db = copperframe_assets::AssetDatabase::scan(&self.session.project);
                match self.resources.load(
                    &self.session.project,
                    &self.session.db,
                    id,
                    &self.renderer,
                    &self.gpu.device,
                    &self.gpu.queue,
                ) {
                    Some(err) => self.session.error(err),
                    None => {
                        let label = self
                            .session
                            .db
                            .get(id)
                            .map_or(id.to_string(), |e| e.relative.clone());
                        self.session.info(format!("reimported {label}"));
                    }
                }
            }
            Action::AssetsChanged => self.resources.retry_unloaded(),
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // 0. Output of the game process and background jobs.
        self.tools.poll(&mut self.session);
        if self.tools.take_game_failed() {
            self.ui.reporting.check_for_crashes();
        }

        // 1. UI (may edit the scene).
        let raw = self.egui_state.take_egui_input(&self.window);
        let mut output = None;
        let mut full = self.egui_ctx.run_ui(raw, |ui| {
            output = Some(ui::draw(
                ui,
                UiContext {
                    session: &mut self.session,
                    tools: &mut self.tools,
                    state: &mut self.ui,
                    camera: &mut self.camera,
                    resources: &self.resources,
                    dt,
                },
            ));
            self.splash.show(ui.ctx());
        });
        self.egui_state
            .handle_platform_output(&self.window, std::mem::take(&mut full.platform_output));
        // Texture uploads are applied now; frees wait until after this
        // frame's draw (or happen at once if nothing is drawn).
        for (id, deltas) in full.textures_delta.set.drain() {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.gpu.device, &self.gpu.queue, id, &delta);
            }
        }
        let to_free: Vec<egui::TextureId> = full.textures_delta.free.drain().collect();
        let free_now = |renderer: &mut egui_wgpu::Renderer| {
            for id in &to_free {
                renderer.free_texture(id);
            }
        };
        let output = output.unwrap_or(ui::UiOutput {
            viewport: None,
            actions: Vec::new(),
        });
        for action in output.actions {
            self.handle(action, event_loop);
        }
        if self.quit {
            free_now(&mut self.egui_renderer);
            return false;
        }
        // 2. Assets the scene now references.
        self.sync_assets();
        self.update_title();
        if self.minimized {
            free_now(&mut self.egui_renderer);
            return false;
        }

        // 3. Render.
        let frame = match self.presenter.acquire(&self.gpu) {
            FrameAcquire::Ready(frame) => frame,
            FrameAcquire::Skip => {
                free_now(&mut self.egui_renderer);
                return false;
            }
            FrameAcquire::Lost => {
                free_now(&mut self.egui_renderer);
                let (instance, window) = (&self.gpu.instance, self.window.clone());
                if let Err(e) = self
                    .presenter
                    .recreate_surface(&self.gpu, || instance.create_surface(window))
                {
                    tracing::error!("could not recreate the rendering surface: {e}");
                }
                return false;
            }
        };
        let output_size = self.presenter.size();
        let device = &self.gpu.device;
        let queue = &self.gpu.queue;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("editor frame"),
        });
        let output_view = frame.texture.create_view(&Default::default());
        let style = self.session.scene.settings.style.clone();
        let geometry = output
            .viewport
            .and_then(|panel| ViewportGeometry::new(panel, &style));
        let mut stats = (0usize, 0u64);
        let view_ok = geometry.is_some_and(|g| {
            self.renderer
                .ensure_view_size(device, &mut self.view, g.internal.0, g.internal.1)
                .map_err(|e| tracing::error!("{e}"))
                .is_ok()
        });
        let world = self.session.scene.world_matrices();
        let lighting = lighting_for_scene(&self.session.scene, &world);
        let poses = self
            .resources
            .preview_poses(&self.session.scene, self.started.elapsed().as_secs_f32());
        {
            let draws = self.resources.draws(&self.session.scene, &world, &poses);
            stats.0 = draws.len();
            stats.1 = draws.iter().map(|d| u64::from(d.triangles)).sum();
            let items: Vec<_> = draws.iter().map(|d| d.item).collect();
            if view_ok {
                self.renderer.render_view(
                    device,
                    queue,
                    &mut encoder,
                    &mut self.view,
                    &self.camera.camera,
                    &lighting,
                    &style,
                    &items,
                );
            }
        }
        let slots: Vec<CompositeSlot<'_>> = match (geometry, view_ok) {
            (Some(g), true) => vec![CompositeSlot {
                view: &self.view,
                rect: g.image,
                scissor: g.image,
                style: &style,
            }],
            _ => Vec::new(),
        };
        self.renderer
            .composite(queue, &mut encoder, &output_view, output_size, &slots);

        // 4. egui on top, at native resolution.
        let ppp = full.pixels_per_point;
        let jobs = self.egui_ctx.tessellate(full.shapes, ppp);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [output_size.0, output_size.1],
            pixels_per_point: ppp,
        };
        let extra = self
            .egui_renderer
            .update_buffers(device, queue, &mut encoder, &jobs, &screen);
        let overlay_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.presenter.overlay_format()),
            ..Default::default()
        });
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &overlay_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer.render(&mut pass, &jobs, &screen);
        }
        free_now(&mut self.egui_renderer);
        queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        queue.present(frame);

        self.frames += 1;
        self.fps_window.1 += 1;
        let elapsed = self.fps_window.0.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            self.ui.stats.fps = self.fps_window.1 as f32 / elapsed;
            self.fps_window = (Instant::now(), 0);
        }
        self.ui.stats.draws = stats.0;
        self.ui.stats.triangles = stats.1;
        true
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        let Some(session) = self.session.take() else {
            return;
        };
        match Running::new(event_loop, &self.display, session) {
            Ok(mut r) => {
                // Automated runs (a frame limit) skip the splash.
                if self.config.exit_after_frames.is_some() {
                    r.splash = Splash::disabled();
                }
                r.window.request_redraw();
                self.running = Some(r);
            }
            Err(e) => {
                tracing::error!("{e:#}");
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(app) = self.running.as_mut() else {
            return;
        };
        let response = app.egui_state.on_window_event(&app.window, &event);
        if response.repaint {
            app.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                if app.ui.request_close(&app.session) {
                    event_loop.exit();
                }
            }
            WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::Resized(size) => {
                app.minimized = size.width == 0 || size.height == 0;
                app.presenter.resize(&app.gpu, size.width, size.height);
            }
            WindowEvent::RedrawRequested if event_loop.exiting() => {}
            WindowEvent::RedrawRequested => {
                let presented = app.redraw(event_loop);
                if presented
                    && self
                        .config
                        .exit_after_frames
                        .is_some_and(|n| app.frames >= n)
                {
                    tracing::info!(frames = app.frames, "frame limit reached; exiting");
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(app) = self.running.as_mut() else {
            return;
        };
        if app.minimized {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
            app.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(app) = self.running.take()
            && app.session.is_dirty()
        {
            tracing::warn!("exiting with unsaved changes (discarded on request)");
        }
    }
}
