//! Editor window: winit event loop, egui integration and frame rendering.
//!
//! The window shows either the project hub (no project open) or a
//! [`Workspace`]: one open project with its session, tools and folder
//! watcher. Opening another project replaces the workspace in the same
//! window.
//!
//! Per frame: run the egui UI (which may edit the scene through the
//! session), load newly referenced assets, render the scene into the
//! viewport rectangle through the retro renderer (scene pass at internal
//! resolution, composite letterboxed into the viewport), then draw egui on
//! top at native resolution so editor text stays legible.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use copperframe_render::{
    CompositeSlot, FrameAcquire, GpuContext, Presenter, RenderView, RetroRenderer, wgpu,
};
use copperframe_runtime::branding::Splash;
use copperframe_runtime::scene_gpu::{SceneResources, lighting_for_scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{
    ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle,
};
use winit::window::{Window, WindowId};

use crate::gameplay::GameTools;
use crate::hub::{Hub, HubAction, OpenFailure};
use crate::session::{EditorSession, OpenError};
use crate::ui::{self, Action, UiContext, UiState};
use crate::viewport::{EditorCamera, ViewportGeometry};
use crate::watch::{Changes, Watcher};

#[derive(Debug, Clone)]
pub struct EditorConfig {
    /// The project to open; `None` starts at the project hub.
    pub project: Option<PathBuf>,
    pub scene: Option<PathBuf>,
    /// Replace an editor lock left behind by a crash.
    pub steal_lock: bool,
    /// Exit after this many frames (automated smoke runs).
    pub exit_after_frames: Option<u64>,
}

pub fn run(config: EditorConfig) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("could not create the window event loop")?;
    let mut app = App {
        display: event_loop.owned_display_handle(),
        proxy: event_loop.create_proxy(),
        config,
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
    /// Wakes the idle event loop (the folder watcher).
    proxy: EventLoopProxy<()>,
    config: EditorConfig,
    running: Option<Running>,
    error: Option<anyhow::Error>,
}

/// One open project.
struct Workspace {
    session: EditorSession,
    tools: GameTools,
    ui: UiState,
    camera: EditorCamera,
    watcher: Watcher,
}

impl Workspace {
    fn open(
        path: &Path,
        steal_lock: bool,
        scene: Option<&Path>,
        proxy: &EventLoopProxy<()>,
    ) -> Result<Self, OpenError> {
        let mut session = EditorSession::open(path, scene, true, steal_lock)?;
        let camera = EditorCamera::from_start(&session.scene.settings.camera);
        let tools = GameTools::new(&mut session);
        let mut ui = UiState::default();
        if let Some((dock, prefs)) =
            crate::dock::layout_path().and_then(|p| crate::dock::load_layout(&p))
        {
            ui.dock = dock;
            ui.prefs = prefs;
        }
        ui.reporting.check_for_crashes();
        let proxy = proxy.clone();
        let watcher = Watcher::start(
            &session.project.root,
            &[
                copperframe_assets::project::ASSETS_DIR,
                copperframe_assets::project::SCENES_DIR,
            ],
            move || {
                let _ = proxy.send_event(());
            },
        );
        Ok(Self {
            session,
            tools,
            ui,
            camera,
            watcher,
        })
    }
}

impl Drop for Workspace {
    /// Remembers the panel layout and toolbar settings for next time.
    fn drop(&mut self) {
        let Some(path) = crate::dock::layout_path() else {
            return;
        };
        if let Err(e) = crate::dock::save_layout(&self.ui.dock, &self.ui.prefs, &path) {
            tracing::warn!("could not save the editor layout: {e}");
        }
    }
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
    hub: Hub,
    /// The open project; `None` shows the hub.
    workspace: Option<Workspace>,
    proxy: EventLoopProxy<()>,
    last_frame: Instant,
    /// Clock for the viewport's animation preview.
    started: Instant,
    minimized: bool,
    frames: u64,
    fps_window: (Instant, u32),
    quit: bool,
    splash: Splash,
    /// When the next frame is needed; `None` waits for input.
    next_frame: Option<Instant>,
}

/// How often the editor wakes while a game or background job may be
/// writing to the console.
const BACKGROUND_POLL: Duration = Duration::from_millis(100);

/// Frame interval of the viewport's animation preview (30 Hz).
const PREVIEW_FRAME: Duration = Duration::from_micros(33_333);

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        display: &OwnedDisplayHandle,
        proxy: EventLoopProxy<()>,
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
        crate::theme::apply(&egui_ctx);
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
            hub: Hub::new(Hub::default_store()),
            workspace: None,
            proxy,
            last_frame: Instant::now(),
            started: Instant::now(),
            minimized: size.width == 0 || size.height == 0,
            frames: 0,
            fps_window: (Instant::now(), 0),
            quit: false,
            splash: Splash::new(),
            next_frame: Some(Instant::now()),
        };
        running.update_title();
        Ok(running)
    }

    fn update_title(&self) {
        let title = match &self.workspace {
            None => "Copperframe Engine — Projects".to_owned(),
            Some(ws) => {
                let dirty = if ws.session.is_dirty() { "*" } else { "" };
                format!(
                    "{dirty}{} — {} — Copperframe Engine editor",
                    ws.session.project.relative(&ws.session.scene_path),
                    ws.session.project.file.name
                )
            }
        };
        if self.window.title() != title {
            self.window.set_title(&title);
        }
    }

    /// Decides when the next frame is needed (see ARCHITECTURE "Editor").
    fn schedule_next_frame(&mut self, now: Instant, egui_delay: Duration) {
        let Some(ws) = self.workspace.as_mut() else {
            let delay = if self.splash.is_active() {
                Duration::ZERO
            } else {
                egui_delay
            };
            self.next_frame = now.checked_add(delay);
            return;
        };
        let interactive = self.splash.is_active() || ws.ui.is_animating();
        let mut delay = if interactive {
            Duration::ZERO
        } else {
            egui_delay
        };
        if self.resources.has_animation_preview(&ws.session.scene) {
            // The preview is ambient motion: cap it, more so in the background.
            let rate = if self.window.has_focus() {
                PREVIEW_FRAME
            } else {
                BACKGROUND_POLL
            };
            delay = delay.min(rate);
        }
        if ws.tools.is_playing() || ws.tools.busy_label().is_some() {
            delay = delay.min(BACKGROUND_POLL);
        }
        let continuous = delay < BACKGROUND_POLL;
        let was_continuous = std::mem::replace(&mut ws.ui.stats.continuous, continuous);
        self.next_frame = if was_continuous && !continuous {
            // One more frame so the status bar shows "idle".
            Some(now)
        } else {
            now.checked_add(delay)
        };
    }

    fn sync_assets(&mut self) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let messages = self.resources.sync(
            &ws.session.project,
            &ws.session.db,
            &ws.session.scene,
            &self.renderer,
            &self.gpu.device,
            &self.gpu.queue,
        );
        for m in messages {
            ws.session.error(m);
        }
    }

    /// Opens a project in this window (closing the current one), or shows
    /// why it could not be opened on the hub.
    fn open_project(&mut self, path: &Path, steal_lock: bool, scene: Option<&Path>) {
        // Release the current project's lock and game first: reopening the
        // same project must not find our own lock.
        self.workspace = None;
        match Workspace::open(path, steal_lock, scene, &self.proxy) {
            Ok(ws) => {
                self.hub
                    .opened(&ws.session.project.root, &ws.session.project.file.name);
                self.resources =
                    SceneResources::new(&self.renderer, &self.gpu.device, &self.gpu.queue);
                self.workspace = Some(ws);
            }
            Err(e) => {
                let locked = matches!(
                    e,
                    OpenError::Project(copperframe_assets::ProjectError::Locked { .. })
                );
                let message = match &e {
                    OpenError::Project(copperframe_assets::ProjectError::NotAProject(p)) => {
                        format!(
                            "{} is not a Copperframe project (it has no project.json). Choose the \
                         project's folder, or create a new project.",
                            p.display()
                        )
                    }
                    OpenError::Project(copperframe_assets::ProjectError::Locked { .. }) => {
                        format!(
                            "{} is already open in another editor window. Close it there, or \
                             open it anyway if that editor crashed.",
                            path.display()
                        )
                    }
                    other => other.to_string(),
                };
                tracing::error!("could not open {}: {e}", path.display());
                self.hub.failure = Some(OpenFailure {
                    path: path.to_path_buf(),
                    message,
                    locked,
                });
            }
        }
    }

    fn handle(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
            Action::OpenProject(path) => self.open_project(&path, false, None),
            Action::CloseProject => self.workspace = None,
            Action::NewProject => {
                self.workspace = None;
                self.hub.start_new_project();
            }
            Action::Reimport(id) => self.reimport(id),
            Action::AssetsChanged => self.resources.retry_unloaded(),
            Action::NewScene {
                path,
                name,
                template,
            } => {
                let Some(ws) = self.workspace.as_mut() else {
                    return;
                };
                if let Err(e) = ws.session.new_scene_at(&path, &name, template) {
                    ws.session.error(e);
                    return;
                }
                ws.session
                    .info(format!("created {}", ws.session.project.relative(&path)));
                self.handle(Action::OpenScene(path), event_loop);
            }
            Action::OpenScene(path) => {
                let Some(ws) = self.workspace.as_mut() else {
                    return;
                };
                // File > New Scene has already switched.
                if ws.session.scene_path != path
                    && let Err(e) = ws.session.open_scene(&path)
                {
                    ws.session.error(e.to_string());
                    return;
                }
                ws.camera = EditorCamera::from_start(&ws.session.scene.settings.camera);
                ws.ui.disk_changed = None;
                ws.ui.assets.clear_thumbnails();
            }
        }
    }

    fn reimport(&mut self, id: copperframe_core::AssetId) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        ws.ui.assets.clear_thumbnails();
        // Pick up edited sidecar settings too.
        ws.session.db = copperframe_assets::AssetDatabase::scan(&ws.session.project);
        match self.resources.load(
            &ws.session.project,
            &ws.session.db,
            id,
            &self.renderer,
            &self.gpu.device,
            &self.gpu.queue,
        ) {
            Some(err) => ws.session.error(err),
            None => {
                let label = ws
                    .session
                    .db
                    .get(id)
                    .map_or(id.to_string(), |e| e.relative.clone());
                ws.session.info(format!("reimported {label}"));
            }
        }
    }

    /// Applies what the folder watcher saw: registers new assets,
    /// reimports changed ones in place, and reloads (or offers to reload)
    /// the open scene when another program changed it.
    fn apply_changes(&mut self, changes: Changes) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let assets_prefix = format!("{}/", copperframe_assets::project::ASSETS_DIR);
        let asset_changes = changes.all().any(|p| p.starts_with(&assets_prefix));
        let scene_rel = ws.session.project.relative(&ws.session.scene_path);
        let scene_changed =
            changes.modified.contains(&scene_rel) || changes.added.contains(&scene_rel);

        let mut reimport = Vec::new();
        if asset_changes {
            ws.ui.assets.clear_thumbnails();
            let before = ws.session.db.entries.len();
            ws.session.rescan_assets();
            ws.ui.audio.forget_missing(&ws.session);
            // Sidecars the rescan wrote are the editor's own change.
            ws.watcher.accept_own_changes();
            let added = ws.session.db.entries.len().saturating_sub(before);
            for rel in &changes.modified {
                // A changed source file, or a sidecar edited by hand.
                let source = rel.strip_suffix(".meta.json").unwrap_or(rel);
                if let Some(entry) = ws.session.db.entries.iter().find(|e| e.relative == source)
                    && self.resources.status(entry.id).is_some()
                    && !reimport.contains(&entry.id)
                {
                    reimport.push(entry.id);
                }
            }
            let removed: Vec<&String> = changes
                .removed
                .iter()
                .filter(|p| p.starts_with(&assets_prefix) && !p.ends_with(".meta.json"))
                .collect();
            for r in &removed {
                ws.session.warn(format!(
                    "{r} was deleted; entities using it show as missing"
                ));
            }
            if added > 0 || !removed.is_empty() {
                self.resources.retry_unloaded();
            }
        }
        if scene_changed {
            match copperframe_scene::load_scene(&ws.session.scene_path) {
                Ok(disk) if disk.revision != ws.session.disk_revision() => {
                    if ws.session.is_dirty() {
                        ws.ui.disk_changed = Some(disk.revision);
                    } else if let Err(e) = ws.session.reload() {
                        ws.session.error(e.to_string());
                    } else {
                        ws.session.info(format!(
                            "{scene_rel} was changed by another program; reloaded it"
                        ));
                    }
                }
                Ok(_) => {}
                // Half-written or broken: say so, keep what is open.
                Err(e) => ws.session.warn(format!(
                    "{scene_rel} changed on disk but could not be read: {e}"
                )),
            }
        }
        for id in reimport {
            self.reimport(id);
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // 0. Output of the game process and background jobs; files other
        // programs changed.
        if let Some(ws) = self.workspace.as_mut() {
            ws.tools.poll(&mut ws.session);
            if ws.tools.take_game_failed() {
                ws.ui.reporting.check_for_crashes();
            }
            if let Some(changes) = ws.watcher.take() {
                self.apply_changes(changes);
            }
        }

        // 1. UI (may edit the scene).
        let raw = self.egui_state.take_egui_input(&self.window);
        let mut output = None;
        let mut hub_action = None;
        let mut full = self.egui_ctx.run_ui(raw, |ui| {
            match self.workspace.as_mut() {
                Some(ws) => {
                    output = Some(ui::draw(
                        ui,
                        UiContext {
                            session: &mut ws.session,
                            tools: &mut ws.tools,
                            state: &mut ws.ui,
                            camera: &mut ws.camera,
                            resources: &self.resources,
                            recent: &self.hub.recent.projects,
                            dt,
                        },
                    ));
                }
                None => hub_action = self.hub.draw(ui),
            }
            self.splash.show(ui.ctx());
        });
        self.egui_state
            .handle_platform_output(&self.window, std::mem::take(&mut full.platform_output));
        let repaint_delay = full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);
        self.schedule_next_frame(now, repaint_delay);
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
        match hub_action {
            Some(HubAction::Open { path, steal_lock }) => {
                self.open_project(&path, steal_lock, None)
            }
            Some(HubAction::Quit) => self.handle(Action::Quit, event_loop),
            None => {}
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
        let style = self
            .workspace
            .as_ref()
            .map(|ws| {
                ws.ui.prefs.look.apply(
                    &ws.ui
                        .project_settings
                        .style_settings(&ws.session)
                        .scene_style(&ws.session.scene.settings),
                )
            })
            .unwrap_or_default();
        let geometry = output
            .viewport
            .filter(|_| self.workspace.is_some())
            .and_then(|panel| {
                let frame = self.workspace.as_ref()?.ui.prefs.frame;
                ViewportGeometry::new(panel, &style, frame)
            });
        let mut stats = (0usize, 0u64);
        let view_ok = geometry.is_some_and(|g| {
            self.renderer
                .ensure_view_size(device, &mut self.view, g.internal.0, g.internal.1)
                .map_err(|e| tracing::error!("{e}"))
                .is_ok()
        });
        if let Some(ws) = &self.workspace {
            let scene = &ws.session.scene;
            let world = scene.world_matrices();
            let lighting = lighting_for_scene(scene, &world);
            let poses = self
                .resources
                .preview_poses(scene, self.started.elapsed().as_secs_f32());
            let draws = self.resources.draws(scene, &world, &poses);
            stats.0 = draws.len();
            stats.1 = draws.iter().map(|d| u64::from(d.triangles)).sum();
            let items: Vec<_> = draws.iter().map(|d| d.item).collect();
            if view_ok {
                self.renderer.render_view(
                    device,
                    queue,
                    &mut encoder,
                    &mut self.view,
                    &ws.camera.camera,
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
        let fps = self.fps_window.1 as f32 / elapsed.max(1e-3);
        if elapsed >= 0.5 {
            self.fps_window = (Instant::now(), 0);
        }
        if let Some(ws) = self.workspace.as_mut() {
            if elapsed >= 0.5 {
                ws.ui.stats.fps = fps;
            }
            ws.ui.stats.draws = stats.0;
            ws.ui.stats.triangles = stats.1;
        }
        true
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match Running::new(event_loop, &self.display, self.proxy.clone()) {
            Ok(mut r) => {
                // Automated runs (a frame limit) skip the splash.
                if self.config.exit_after_frames.is_some() {
                    r.splash = Splash::disabled();
                }
                if let Some(project) = self.config.project.clone() {
                    r.open_project(
                        &project,
                        self.config.steal_lock,
                        self.config.scene.as_deref(),
                    );
                    // Automated runs fail instead of waiting at the hub.
                    if self.config.exit_after_frames.is_some()
                        && let Some(failure) = &r.hub.failure
                    {
                        self.error = Some(anyhow::anyhow!(
                            "could not open project {}: {}",
                            project.display(),
                            failure.message
                        ));
                        event_loop.exit();
                        return;
                    }
                }
                r.update_title();
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
        // egui reports every RedrawRequested as needing a repaint; acting on
        // that would redraw forever. Its real repaint requests arrive as
        // the frame's repaint delay (`schedule_next_frame`).
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            app.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                let may_close = match app.workspace.as_mut() {
                    Some(ws) => ws.ui.request_close(&ws.session),
                    None => true,
                };
                if may_close {
                    event_loop.exit();
                } else {
                    app.window.request_redraw();
                }
            }
            WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::Focused(_) | WindowEvent::Occluded(false) => {
                app.window.request_redraw();
            }
            WindowEvent::Resized(size) => {
                app.window.request_redraw();
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
        if self.config.exit_after_frames.is_some() && !app.minimized {
            // Smoke runs count frames: keep drawing.
            event_loop.set_control_flow(ControlFlow::Poll);
            app.window.request_redraw();
            return;
        }
        match app.next_frame {
            _ if app.minimized => event_loop.set_control_flow(ControlFlow::Wait),
            Some(at) if at <= Instant::now() => {
                event_loop.set_control_flow(ControlFlow::Poll);
                app.window.request_redraw();
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        // The folder watcher saw changes.
        if let Some(app) = &self.running {
            app.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(app) = self.running.take()
            && app
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.session.is_dirty())
        {
            tracing::warn!("exiting with unsaved changes (discarded on request)");
        }
    }
}
