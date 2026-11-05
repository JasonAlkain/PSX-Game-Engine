//! The standalone game window: title screen, play, pause, settings (volume,
//! mouse, display, key rebinding), the end screen, and the HUD.
//!
//! Per frame: run the menu UI; while playing, advance the fixed-step
//! simulation with the input gathered since the last tick; render the
//! scene with transforms and camera interpolated between the last two
//! ticks; draw egui on top.
//!
//! Settings and saves go to the user's directories ([`crate::settings`]).
//! Play mode from the editor (`skip_title`) never writes the save file.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use copperframe_assets::play_control::ControlCommand;
use copperframe_assets::project::Project;
use copperframe_core::FixedStep;
use copperframe_render::{
    Camera, CompositeSlot, FrameAcquire, GpuContext, Lighting, Presenter, RenderView,
    RetroRenderer, ViewLayout, wgpu,
};
use copperframe_runtime::branding::Splash;
use copperframe_runtime::scene_gpu::{SceneResources, scene_lighting};
use glam::{Mat4, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::actions::{ActionInput, Bindings, Button, MouseKey, names};
use crate::api::Game;
use crate::audio::Audio;
use crate::control::{self, ControlInput, Incoming, Pacing};
use crate::settings::{GraphicsStyle, SaveData, UserDirs, UserSettings};
use crate::sim::{Outcome, Simulation, TICK_HZ};

pub struct PlayerOptions {
    pub project: Project,
    /// Scene to start (project-relative or absolute).
    pub scene: PathBuf,
    /// Start playing at once and never write saves (editor play mode).
    pub skip_title: bool,
    /// Exit after presenting this many frames (smoke tests).
    pub exit_after_frames: Option<u64>,
    /// Settings/save location; `None` = the user's per-game directories.
    pub dirs: Option<UserDirs>,
    /// Editor commands (pause, resume, step, quit), with `--control stdin`.
    pub control: Option<ControlInput>,
}

/// Opens the game window and runs until the player quits.
pub fn run(options: PlayerOptions, game: Box<dyn Game>) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("could not create the window event loop")?;
    let mut app = App {
        display: event_loop.owned_display_handle(),
        pending: Some((options, game)),
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

#[derive(Debug, Clone, PartialEq)]
enum Screen {
    Title,
    Playing,
    Paused,
    Settings {
        back: Box<Screen>,
    },
    Finished(String),
    /// The scene cannot be played; the message says why.
    Error(String),
}

enum UiAction {
    NewGame,
    Continue,
    Resume,
    OpenSettings,
    CloseSettings,
    QuitToTitle,
    Quit,
    Restart,
    SettingsChanged,
    StartRebind(String),
    ResetKeys,
}

struct App {
    display: OwnedDisplayHandle,
    pending: Option<(PlayerOptions, Box<dyn Game>)>,
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
    /// Scene whose assets `resources` was last trimmed to.
    resources_scene: Option<PathBuf>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    sim: Simulation,
    start_scene: PathBuf,
    screen: Screen,
    skip_title: bool,
    dirs: Option<UserDirs>,
    settings: UserSettings,
    /// The project's fixed look when players may not change it.
    style_lock: Option<GraphicsStyle>,
    project_bindings: Bindings,
    bindings: Bindings,
    rebinding: Option<String>,
    input: ActionInput,
    stepper: FixedStep,
    alpha: f32,
    last_frame: Instant,
    cursor_captured: bool,
    focused: bool,
    minimized: bool,
    /// Messages shown in the corner for a few seconds (warnings, saves).
    notices: Vec<(String, Instant)>,
    exit_after_frames: Option<u64>,
    frames: u64,
    quit: bool,
    splash: Splash,
    /// Editor control channel and the pause/step state it sets.
    control: Option<ControlInput>,
    pacing: Pacing,
}

const NOTICE_SECS: f32 = 6.0;

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        display: &OwnedDisplayHandle,
        options: PlayerOptions,
        game: Box<dyn Game>,
    ) -> anyhow::Result<Self> {
        let title = game.title();
        let mut warnings = Vec::new();
        let dirs = options.dirs.or_else(|| {
            let d = UserDirs::for_game(&title);
            if d.is_none() {
                warnings.push("no user directory; settings will not be saved".to_owned());
            }
            d
        });
        let defaults = UserSettings::for_project(&options.project.file);
        let settings = dirs
            .as_ref()
            .map(|d| d.load_settings_or(&defaults, &mut warnings))
            .unwrap_or(defaults);
        let window_size = options.project.file.display.clone();
        let style_lock = (!options.project.file.style.player_can_change)
            .then_some(options.project.file.style.default);
        let project_bindings = Bindings::load_project(&options.project.root).unwrap_or_else(|e| {
            warnings.push(format!("{e}; using default keys"));
            Bindings::default()
        });
        let bindings = settings.bindings(&project_bindings, &mut warnings);

        let mut attributes = Window::default_attributes()
            .with_title(title.clone())
            .with_window_icon(copperframe_runtime::branding::window_icon())
            .with_inner_size(LogicalSize::new(window_size.width, window_size.height))
            .with_min_inner_size(LogicalSize::new(320, 240));
        if settings.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the game window")?,
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
        let mut presenter = Presenter::new(&gpu, surface, size.width, size.height)?;
        presenter.set_vsync(&gpu, settings.vsync);
        let renderer = RetroRenderer::new(&gpu.device, presenter.format());
        let view = renderer
            .create_view(&gpu.device, 320, 240)
            .context("could not create the internal render target")?;
        let resources = SceneResources::new(&renderer, &gpu.device, &gpu.queue);

        let egui_ctx = egui::Context::default();
        egui_ctx.set_visuals(egui::Visuals::dark());
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            presenter.overlay_format(),
            egui_wgpu::RendererOptions::default(),
        );

        let (mut audio, audio_warning) = Audio::open_default();
        warnings.extend(audio_warning);
        audio.set_mixer(options.project.file.audio);
        audio.set_volumes(settings.volumes());
        let mut sim = Simulation::new(options.project, game, audio);
        let screen = match sim.load_scene_file(&options.scene) {
            Ok(()) if options.skip_title => Screen::Playing,
            Ok(()) => Screen::Title,
            Err(e) => {
                tracing::error!("{e}");
                Screen::Error(e.to_string())
            }
        };
        warnings.extend(sim.take_warnings());
        let now = Instant::now();
        for w in &warnings {
            tracing::warn!("{w}");
        }
        let mut running = Self {
            window,
            gpu,
            presenter,
            renderer,
            view,
            resources,
            resources_scene: None,
            egui_ctx,
            egui_state,
            egui_renderer,
            sim,
            start_scene: options.scene,
            screen: Screen::Title,
            skip_title: options.skip_title,
            dirs,
            settings,
            style_lock,
            project_bindings,
            bindings,
            rebinding: None,
            input: ActionInput::default(),
            stepper: FixedStep::from_hz(TICK_HZ),
            alpha: 1.0,
            last_frame: now,
            cursor_captured: false,
            focused: true,
            minimized: size.width == 0 || size.height == 0,
            notices: warnings.into_iter().map(|w| (w, now)).collect(),
            exit_after_frames: options.exit_after_frames,
            frames: 0,
            quit: false,
            control: options.control,
            pacing: Pacing::default(),
            // The engine splash shows before the title screen, not when the
            // editor's Play skips the title.
            splash: if options.skip_title {
                Splash::disabled()
            } else {
                Splash::new()
            },
        };
        running.set_screen(screen);
        running.show_script_problems();
        if running.control.is_some() {
            // Tells the editor the game is up and listening.
            control::report(&running.pacing.status(running.sim.tick_count()));
        }
        Ok(running)
    }

    /// Script errors are already in the log; show them in the corner too,
    /// and send them to the editor's Problems panel during Play.
    fn show_script_problems(&mut self) {
        let now = Instant::now();
        for d in self.sim.take_diagnostics() {
            self.notices.push((format!("Script error: {d}"), now));
            if self.control.is_some() {
                control::report_problem(&d);
            }
        }
    }

    fn notice(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("{text}");
        self.notices.push((text, Instant::now()));
    }

    fn set_screen(&mut self, screen: Screen) {
        // Events from loads done in menus are already logged.
        self.sim.take_events();
        let playing = screen == Screen::Playing;
        let was_playing = self.screen == Screen::Playing;
        self.screen = screen;
        self.input.clear();
        if playing != was_playing {
            let paused = !playing || self.pacing.is_paused();
            self.sim.audio_mut().set_paused(paused);
            // Time spent in menus is not simulated.
            self.stepper = FixedStep::from_hz(TICK_HZ);
            self.last_frame = Instant::now();
        }
        self.set_cursor_capture(playing && self.focused && !self.pacing.is_paused());
    }

    /// Applies the editor's commands. Returns true when the game should
    /// close (a `quit` command, or the editor went away).
    fn poll_control(&mut self) -> bool {
        let Some(input) = self.control.as_mut() else {
            return false;
        };
        let mut quit = false;
        for item in input.drain() {
            match item {
                Incoming::Command(ControlCommand::Quit) => {
                    tracing::info!("the editor asked the game to close");
                    quit = true;
                }
                Incoming::Closed => {
                    if !quit {
                        tracing::info!("editor control closed; closing the game");
                    }
                    quit = true;
                }
                Incoming::Command(c) => {
                    let was_paused = self.pacing.is_paused();
                    self.pacing.apply(c);
                    if self.pacing.is_paused() != was_paused {
                        self.editor_pause_changed();
                    }
                    if self.pacing.is_paused() && self.screen == Screen::Paused {
                        // The editor's pause replaces the in-game menu, so
                        // steps run (the menu opens when the window loses
                        // focus to the editor).
                        self.set_screen(Screen::Playing);
                    }
                    if !matches!(self.screen, Screen::Playing) {
                        self.pacing.cancel_steps();
                    }
                    tracing::info!(command = %c.to_line(), tick = self.sim.tick_count(), "editor control");
                    control::report(&self.pacing.status(self.sim.tick_count()));
                }
                Incoming::Invalid(e) => {
                    tracing::warn!("editor control: {e}");
                    control::report(&self.pacing.status(self.sim.tick_count()).with_error(e));
                }
            }
        }
        if quit {
            let mut status = self.pacing.status(self.sim.tick_count());
            status.state = copperframe_assets::play_control::RunState::Quitting;
            control::report(&status);
        }
        quit
    }

    fn editor_pause_changed(&mut self) {
        let paused = self.pacing.is_paused();
        let playing = self.screen == Screen::Playing;
        self.sim.audio_mut().set_paused(paused || !playing);
        self.input.clear();
        // Time spent paused is not simulated.
        self.stepper = FixedStep::from_hz(TICK_HZ);
        self.last_frame = Instant::now();
        self.set_cursor_capture(playing && self.focused && !paused);
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
            if let Err(e) = grabbed {
                tracing::warn!("mouse capture unavailable: {e}");
            }
            self.window.set_cursor_visible(false);
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
        }
        self.cursor_captured = capture;
    }

    fn save_progress(&mut self) {
        if self.skip_title {
            return;
        }
        let (Some(dirs), Some(path)) = (&self.dirs, self.sim.scene_path()) else {
            return;
        };
        let data = SaveData::new(self.sim.project().relative(path), self.sim.state().clone());
        if let Err(e) = dirs.write_save(&data) {
            self.notice(e);
        }
    }

    fn save_settings(&mut self) {
        self.sim.audio_mut().set_volumes(self.settings.volumes());
        self.presenter.set_vsync(&self.gpu, self.settings.vsync);
        let fullscreen = self
            .settings
            .fullscreen
            .then_some(Fullscreen::Borderless(None));
        if self.window.fullscreen().is_some() != self.settings.fullscreen {
            self.window.set_fullscreen(fullscreen);
        }
        if let Some(dirs) = &self.dirs
            && let Err(e) = dirs.save_settings(&self.settings)
        {
            self.notice(e);
        }
    }

    fn handle(&mut self, action: UiAction, event_loop: &ActiveEventLoop) {
        match action {
            UiAction::NewGame | UiAction::Restart => {
                self.sim.set_state(Default::default());
                match self.sim.load_scene_file(&self.start_scene.clone()) {
                    Ok(()) => self.set_screen(Screen::Playing),
                    Err(e) => self.set_screen(Screen::Error(e.to_string())),
                }
            }
            UiAction::Continue => {
                let mut warnings = Vec::new();
                let save = self.dirs.as_ref().and_then(|d| d.load_save(&mut warnings));
                for w in warnings {
                    self.notice(w);
                }
                let Some(save) = save else {
                    self.notice("no saved game");
                    return;
                };
                // State first, so `scene_started` sees the progress.
                self.sim.set_state(save.state);
                match self.sim.load_scene_file(&PathBuf::from(&save.scene)) {
                    Ok(()) => self.set_screen(Screen::Playing),
                    Err(e) => self.notice(format!("could not continue: {e}")),
                }
            }
            UiAction::Resume => self.set_screen(Screen::Playing),
            UiAction::OpenSettings => {
                let back = Box::new(self.screen.clone());
                self.set_screen(Screen::Settings { back });
            }
            UiAction::CloseSettings => {
                self.rebinding = None;
                if let Screen::Settings { back } = self.screen.clone() {
                    self.set_screen(*back);
                }
            }
            UiAction::QuitToTitle => {
                self.save_progress();
                if self.skip_title {
                    self.quit(event_loop);
                    return;
                }
                if let Err(e) = self.sim.load_scene_file(&self.start_scene.clone()) {
                    self.set_screen(Screen::Error(e.to_string()));
                    return;
                }
                self.set_screen(Screen::Title);
            }
            UiAction::Quit => {
                if matches!(self.screen, Screen::Paused) {
                    self.save_progress();
                }
                self.quit(event_loop);
            }
            UiAction::SettingsChanged => self.save_settings(),
            UiAction::StartRebind(a) => self.rebinding = Some(a),
            UiAction::ResetKeys => {
                self.settings.bindings = None;
                self.bindings = self.project_bindings.clone();
                self.save_settings();
            }
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.quit = true;
        self.set_cursor_capture(false);
        event_loop.exit();
    }

    /// A key or mouse button pressed while waiting for a rebind.
    fn rebind(&mut self, button: Button) {
        let Some(action) = self.rebinding.take() else {
            return;
        };
        if button == Button::Key(KeyCode::Escape) && action != names::PAUSE {
            return; // Escape cancels.
        }
        self.bindings.rebind(&action, button);
        // Store the complete map so later project changes cannot leave an
        // action with a button that was moved elsewhere.
        self.settings.bindings = serde_json::from_str(&self.bindings.to_json()).ok();
        self.save_settings();
    }

    fn on_button(&mut self, button: Button, pressed: bool, repeat: bool) {
        tracing::debug!(button = %button.name(), pressed, repeat, screen = ?self.screen, "input");
        if self.rebinding.is_some() {
            if pressed {
                self.rebind(button);
            }
            return;
        }
        if pressed && !repeat && self.bindings.buttons(names::PAUSE).contains(&button) {
            match self.screen {
                Screen::Playing => {
                    self.set_screen(Screen::Paused);
                    return;
                }
                Screen::Paused => {
                    self.set_screen(Screen::Playing);
                    return;
                }
                _ => {}
            }
        }
        if self.screen != Screen::Playing {
            return;
        }
        if pressed {
            self.input.button_down(button, repeat);
        } else {
            self.input.button_up(button);
        }
    }

    fn simulate(&mut self, frame_time: Duration) {
        if self.screen != Screen::Playing {
            return;
        }
        if self.pacing.is_paused() {
            // Paused by the editor: only step ticks run, and the frame
            // shows the newest tick as it is.
            let had_steps = self.pacing.pending_steps() > 0;
            let n = self.pacing.take_step_ticks();
            for _ in 0..n {
                if self.tick_once() {
                    self.pacing.cancel_steps();
                    break;
                }
            }
            self.stepper = FixedStep::from_hz(TICK_HZ);
            self.alpha = 1.0;
            if had_steps && self.pacing.pending_steps() == 0 {
                control::report(&self.pacing.status(self.sim.tick_count()));
            }
            return;
        }
        let plan = self.stepper.advance(frame_time);
        for _ in 0..plan.ticks {
            if self.tick_once() {
                break;
            }
        }
        self.alpha = plan.alpha;
    }

    /// Runs one fixed tick with the input gathered since the last one.
    /// Returns true when the game finished or quit.
    fn tick_once(&mut self) -> bool {
        let input = self.input.begin_tick(
            &self.bindings,
            self.settings.mouse_sensitivity,
            self.settings.invert_y,
        );
        self.sim.tick(&input);
        // The simulation already logs its events.
        self.sim.take_events();
        for w in self.sim.take_warnings() {
            self.notice(w);
        }
        self.show_script_problems();
        let Some(outcome) = self.sim.outcome().cloned() else {
            return false;
        };
        match outcome {
            Outcome::Finished { message } => {
                if let Some(d) = &self.dirs
                    && !self.skip_title
                {
                    let _ = std::fs::remove_file(d.save_path());
                }
                self.set_screen(Screen::Finished(message));
            }
            Outcome::Quit => self.quit = true,
        }
        true
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let now = Instant::now();
        let frame_time = now - self.last_frame;
        self.last_frame = now;
        self.simulate(frame_time);
        if self.quit {
            self.quit(event_loop);
            return false;
        }
        self.notices
            .retain(|(_, t)| t.elapsed().as_secs_f32() < NOTICE_SECS);

        // UI.
        let raw = self.egui_state.take_egui_input(&self.window);
        let mut actions = Vec::new();
        let mut full = {
            let ui = MenuUi {
                screen: &self.screen,
                title: self.sim.title(),
                has_save: !self.skip_title && self.dirs.as_ref().is_some_and(UserDirs::has_save),
                in_editor: self.skip_title,
                settings: &mut self.settings,
                style_choice: self.style_lock.is_none(),
                bindings: &self.bindings,
                rebinding: self.rebinding.as_deref(),
                hud: self.sim.hud(),
                notices: &self.notices,
                editor_paused: self
                    .pacing
                    .is_paused()
                    .then(|| format!("Paused by the editor · tick {}", self.sim.tick_count())),
            };
            let mut ui = ui;
            self.egui_ctx.run_ui(raw, |ctx| {
                ui.draw(ctx, &mut actions);
                self.splash.show(ctx.ctx());
            })
        };
        self.egui_state
            .handle_platform_output(&self.window, std::mem::take(&mut full.platform_output));
        for (id, deltas) in full.textures_delta.set.drain() {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.gpu.device, &self.gpu.queue, id, &delta);
            }
        }
        let to_free: Vec<egui::TextureId> = full.textures_delta.free.drain().collect();
        for a in actions {
            self.handle(a, event_loop);
        }
        let free = |r: &mut egui_wgpu::Renderer| {
            for id in &to_free {
                r.free_texture(id);
            }
        };
        if self.quit || self.minimized {
            free(&mut self.egui_renderer);
            return false;
        }

        // After a scene change, release GPU assets the new scene does not
        // reference (runtime spawns reload what they need).
        if self.sim.scene_path() != self.resources_scene.as_deref() {
            self.resources_scene = self.sim.scene_path().map(PathBuf::from);
            if let Some(scene) = self.sim.scene() {
                let released = self
                    .resources
                    .release_unused(scene.entities.iter().filter_map(|e| e.mesh.as_ref()));
                if released > 0 {
                    tracing::info!(released, "released GPU assets of the previous scene");
                }
            }
        }
        // Assets for whatever is visible now.
        let draws = self.sim.draw_list();
        let messages = self.resources.sync_meshes(
            self.sim.project(),
            self.sim.asset_db(),
            draws.iter().map(|d| &d.mesh),
            &self.renderer,
            &self.gpu.device,
            &self.gpu.queue,
        );
        for m in messages {
            self.notice(m);
        }

        let frame = match self.presenter.acquire(&self.gpu) {
            FrameAcquire::Ready(f) => f,
            FrameAcquire::Skip => {
                free(&mut self.egui_renderer);
                return false;
            }
            FrameAcquire::Lost => {
                free(&mut self.egui_renderer);
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
            label: Some("game frame"),
        });
        let output_view = frame.texture.create_view(&Default::default());
        let alpha = if self.screen == Screen::Playing {
            self.alpha
        } else {
            1.0
        };
        let look = self.style_lock.unwrap_or(self.settings.graphics_style);
        let (style, lighting) = match self.sim.scene() {
            Some(s) => {
                let lights = self.sim.lights(alpha);
                let shadows = self.sim.shadows(alpha);
                (
                    look.apply(&self.sim.project().file.style.scene_style(&s.settings)),
                    scene_lighting(
                        &s.settings.lighting,
                        lights.iter().map(|(m, l)| (*m, l)),
                        shadows.iter().map(|(m, s)| (*m, s)),
                    ),
                )
            }
            None => (
                look.apply(&copperframe_core::RetroStyle::ps1()),
                Lighting::default(),
            ),
        };
        let cam = self.sim.camera(alpha);
        let camera = Camera {
            position: cam.position,
            yaw: cam.yaw,
            pitch: cam.pitch,
            fov_y: cam.fov_deg.to_radians(),
            ..Camera::default()
        };
        let layout = ViewLayout::for_style(output_size, &style);
        let view_ok = layout.is_some_and(|l| {
            self.renderer
                .ensure_view_size(device, &mut self.view, l.internal.0, l.internal.1)
                .map_err(|e| tracing::error!("{e}"))
                .is_ok()
        });
        if view_ok {
            let items = draws.iter().map(|d| {
                (
                    d.id,
                    &d.mesh,
                    blend(d.previous, d.current, alpha),
                    d.joints.as_deref(),
                )
            });
            let scene_draws = self.resources.draws_from(items);
            let items: Vec<_> = scene_draws.iter().map(|d| d.item).collect();
            self.renderer.render_view(
                device,
                queue,
                &mut encoder,
                &mut self.view,
                &camera,
                &lighting,
                &style,
                &items,
            );
        }
        let slots: Vec<CompositeSlot<'_>> = match (layout, view_ok) {
            (Some(l), true) => vec![CompositeSlot {
                view: &self.view,
                rect: l.placement.rect,
                scissor: l.placement.rect,
                style: &style,
            }],
            _ => Vec::new(),
        };
        self.renderer
            .composite(queue, &mut encoder, &output_view, output_size, &slots);

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
                label: Some("game ui"),
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
        free(&mut self.egui_renderer);
        queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        queue.present(frame);
        self.frames += 1;
        true
    }
}

/// Interpolates two affine transforms (scale, rotation, translation).
fn blend(previous: Mat4, current: Mat4, t: f32) -> Mat4 {
    if t >= 1.0 || previous == current {
        return current;
    }
    let (sa, ra, ta) = previous.to_scale_rotation_translation();
    let (sb, rb, tb) = current.to_scale_rotation_translation();
    Mat4::from_scale_rotation_translation(sa.lerp(sb, t), ra.slerp(rb, t), ta.lerp(tb, t))
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        let Some((options, game)) = self.pending.take() else {
            return;
        };
        match Running::new(event_loop, &self.display, options, game) {
            Ok(r) => {
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
        let playing = app.screen == Screen::Playing;
        // While playing, input goes to the game, not the (hidden) menus.
        if !playing || !matches!(event, WindowEvent::KeyboardInput { .. }) {
            let r = app.egui_state.on_window_event(&app.window, &event);
            if r.repaint {
                app.window.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                if app.screen == Screen::Playing || app.screen == Screen::Paused {
                    app.save_progress();
                }
                app.quit(event_loop);
            }
            WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::Resized(size) => {
                app.minimized = size.width == 0 || size.height == 0;
                app.presenter.resize(&app.gpu, size.width, size.height);
            }
            WindowEvent::Focused(focused) => {
                app.focused = focused;
                app.input.clear();
                if !focused && app.screen == Screen::Playing && !app.pacing.is_paused() {
                    // Losing focus pauses, so the game never runs unseen.
                    app.set_screen(Screen::Paused);
                }
                if !focused {
                    app.set_cursor_capture(false);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && Button::bindable(code)
                {
                    app.on_button(
                        Button::Key(code),
                        event.state == ElementState::Pressed,
                        event.repeat,
                    );
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(m) = MouseKey::from_winit(button) {
                    // A click on a menu button must not also become a rebind.
                    let over_ui = app.egui_ctx.egui_wants_pointer_input();
                    if app.screen == Screen::Playing || (app.rebinding.is_some() && !over_ui) {
                        app.on_button(Button::Mouse(m), state == ElementState::Pressed, false);
                    }
                }
            }
            WindowEvent::RedrawRequested if event_loop.exiting() => {}
            WindowEvent::RedrawRequested => {
                let presented = app.redraw(event_loop);
                if presented && app.exit_after_frames.is_some_and(|n| app.frames >= n) {
                    tracing::info!(frames = app.frames, "frame limit reached; exiting");
                    app.quit(event_loop);
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let (Some(app), DeviceEvent::MouseMotion { delta }) = (self.running.as_mut(), event)
            && app.screen == Screen::Playing
            && app.focused
        {
            app.input
                .mouse_moved(Vec2::new(delta.0 as f32, delta.1 as f32));
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(app) = self.running.as_mut() else {
            return;
        };
        if app.poll_control() {
            app.quit(event_loop);
            return;
        }
        if app.minimized {
            // Keep reading editor commands while minimised.
            event_loop.set_control_flow(if app.control.is_some() {
                ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100))
            } else {
                ControlFlow::Wait
            });
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
            app.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(mut app) = self.running.take() {
            app.set_cursor_capture(false);
            app.sim.shutdown();
            tracing::info!(
                frames = app.frames,
                ticks = app.sim.tick_count(),
                "game closed"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Menus and HUD

struct MenuUi<'a> {
    screen: &'a Screen,
    title: String,
    has_save: bool,
    in_editor: bool,
    settings: &'a mut UserSettings,
    /// Offer the graphics style choice (the project allows it).
    style_choice: bool,
    bindings: &'a Bindings,
    rebinding: Option<&'a str>,
    hud: &'a crate::sim::Hud,
    notices: &'a [(String, Instant)],
    /// "Paused by the editor" banner text, while paused from the editor.
    editor_paused: Option<String>,
}

const MENU_ACTIONS: [(&str, &str); 8] = [
    (names::MOVE_FORWARD, "Move forward"),
    (names::MOVE_BACK, "Move back"),
    (names::MOVE_LEFT, "Move left"),
    (names::MOVE_RIGHT, "Move right"),
    (names::RUN, "Run"),
    (names::JUMP, "Jump"),
    (names::INTERACT, "Interact"),
    (names::PAUSE, "Pause"),
];

impl MenuUi<'_> {
    fn draw(&mut self, ui: &mut egui::Ui, out: &mut Vec<UiAction>) {
        let ctx = ui.ctx().clone();
        match self.screen {
            Screen::Playing => {
                self.hud(&ctx);
                self.editor_banner(&ctx);
            }
            Screen::Title => self.panel(
                &ctx,
                &self.title.clone(),
                true,
                |me, ui, out| {
                    if big(ui, "New game") {
                        out.push(UiAction::NewGame);
                    }
                    if me.has_save && big(ui, "Continue") {
                        out.push(UiAction::Continue);
                    }
                    if big(ui, "Settings") {
                        out.push(UiAction::OpenSettings);
                    }
                    if big(ui, "Quit") {
                        out.push(UiAction::Quit);
                    }
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("PS1/PS2-style — Copperframe Engine")
                            .small()
                            .weak(),
                    );
                },
                out,
            ),
            Screen::Paused => self.panel(
                &ctx,
                "Paused",
                true,
                |me, ui, out| {
                    if big(ui, "Resume") {
                        out.push(UiAction::Resume);
                    }
                    if big(ui, "Restart") {
                        out.push(UiAction::Restart);
                    }
                    if big(ui, "Settings") {
                        out.push(UiAction::OpenSettings);
                    }
                    if me.in_editor {
                        if big(ui, "Stop") {
                            out.push(UiAction::Quit);
                        }
                    } else {
                        if big(ui, "Quit to title") {
                            out.push(UiAction::QuitToTitle);
                        }
                        if big(ui, "Quit game") {
                            out.push(UiAction::Quit);
                        }
                    }
                },
                out,
            ),
            Screen::Finished(message) => {
                let message = message.clone();
                self.panel(
                    &ctx,
                    "The end",
                    true,
                    |me, ui, out| {
                        ui.label(egui::RichText::new(&message).size(20.0));
                        ui.add_space(12.0);
                        if big(ui, "Play again") {
                            out.push(UiAction::Restart);
                        }
                        if !me.in_editor && big(ui, "Title screen") {
                            out.push(UiAction::QuitToTitle);
                        }
                        if big(ui, "Quit") {
                            out.push(UiAction::Quit);
                        }
                    },
                    out,
                )
            }
            Screen::Error(message) => {
                let message = message.clone();
                self.panel(
                    &ctx,
                    "This scene cannot be played",
                    true,
                    |_, ui, out| {
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                ui.label(&message);
                            });
                        if big(ui, "Quit") {
                            out.push(UiAction::Quit);
                        }
                    },
                    out,
                )
            }
            Screen::Settings { .. } => self.panel(
                &ctx,
                "Settings",
                false,
                |me, ui, out| {
                    me.settings_body(ui, out);
                },
                out,
            ),
        }
        self.notices(&ctx);
    }

    fn panel(
        &mut self,
        ctx: &egui::Context,
        heading: &str,
        dim: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui, &mut Vec<UiAction>),
        out: &mut Vec<UiAction>,
    ) {
        if dim {
            let screen = ctx.content_rect();
            ctx.layer_painter(egui::LayerId::background()).rect_filled(
                screen,
                0.0,
                egui::Color32::from_black_alpha(140),
            );
        }
        egui::Window::new(heading)
            .id(egui::Id::unique("menu"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .min_width(320.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(egui::RichText::new(heading).size(28.0).strong());
                    ui.add_space(16.0);
                    body(self, ui, out);
                });
            });
    }

    fn settings_body(&mut self, ui: &mut egui::Ui, out: &mut Vec<UiAction>) {
        let s = &mut *self.settings;
        let mut changed = false;
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                for (label, v) in [
                    ("Master volume", &mut s.master_volume),
                    ("Music volume", &mut s.music_volume),
                    ("Effects volume", &mut s.effects_volume),
                ] {
                    ui.label(label);
                    changed |= ui
                        .add(egui::Slider::new(v, 0.0..=1.0).show_value(false))
                        .changed();
                    ui.end_row();
                }
                ui.label("Mouse sensitivity");
                changed |= ui
                    .add(egui::Slider::new(&mut s.mouse_sensitivity, 0.02..=0.5).logarithmic(true))
                    .changed();
                ui.end_row();
                ui.label("Invert mouse Y");
                changed |= ui.checkbox(&mut s.invert_y, "").changed();
                ui.end_row();
                ui.label("Fullscreen");
                changed |= ui.checkbox(&mut s.fullscreen, "").changed();
                ui.end_row();
                ui.label("Vertical sync");
                changed |= ui.checkbox(&mut s.vsync, "").changed();
                ui.end_row();
                if self.style_choice {
                    ui.label("Graphics style");
                    ui.horizontal(|ui| {
                        for g in GraphicsStyle::ALL {
                            changed |= ui
                                .selectable_value(&mut s.graphics_style, g, g.label())
                                .changed();
                        }
                    });
                    ui.end_row();
                }
            });
        if changed {
            out.push(UiAction::SettingsChanged);
        }
        ui.add_space(12.0);
        ui.heading("Controls");
        egui::Grid::new("keys")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                for (action, label) in MENU_ACTIONS {
                    ui.label(label);
                    let text = if self.rebinding == Some(action) {
                        "press a key… (Esc cancels)".to_owned()
                    } else {
                        let b: Vec<String> = self
                            .bindings
                            .buttons(action)
                            .iter()
                            .map(|b| b.label())
                            .collect();
                        if b.is_empty() {
                            "unbound".into()
                        } else {
                            b.join(", ")
                        }
                    };
                    if ui.button(text).clicked() && self.rebinding.is_none() {
                        out.push(UiAction::StartRebind(action.to_owned()));
                    }
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Reset controls").clicked() {
                out.push(UiAction::ResetKeys);
            }
            if ui.button("Back").clicked() {
                out.push(UiAction::CloseSettings);
            }
        });
    }

    fn hud(&self, ctx: &egui::Context) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("hud"),
        ));
        let rect = ctx.content_rect();
        let c = rect.center();
        // Crosshair.
        let stroke = egui::Stroke::new(1.5, egui::Color32::from_white_alpha(170));
        painter.line_segment([c - egui::vec2(6.0, 0.0), c + egui::vec2(6.0, 0.0)], stroke);
        painter.line_segment([c - egui::vec2(0.0, 6.0), c + egui::vec2(0.0, 6.0)], stroke);
        let text = |pos: egui::Pos2, anchor: egui::Align2, s: &str, size: f32| {
            let font = egui::FontId::proportional(size);
            let galley_rect = painter.text(
                pos + egui::vec2(2.0, 2.0),
                anchor,
                s,
                font.clone(),
                egui::Color32::BLACK,
            );
            let _ = galley_rect;
            painter.text(pos, anchor, s, font, egui::Color32::WHITE);
        };
        if let Some(p) = &self.hud.prompt {
            text(c + egui::vec2(0.0, 48.0), egui::Align2::CENTER_TOP, p, 22.0);
        }
        if let Some((m, _)) = &self.hud.message {
            text(
                egui::pos2(c.x, rect.bottom() - 80.0),
                egui::Align2::CENTER_BOTTOM,
                m,
                24.0,
            );
        }
    }

    fn editor_banner(&self, ctx: &egui::Context) {
        let Some(text) = &self.editor_paused else {
            return;
        };
        egui::Area::new(egui::Id::unique("editor-paused"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).strong()).extend());
                });
            });
    }

    fn notices(&self, ctx: &egui::Context) {
        if self.notices.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("notices"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 8.0))
            .interactable(false)
            .show(ctx, |ui| {
                for (n, _) in self.notices {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(n);
                    });
                }
            });
    }
}

fn big(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized(
        [240.0, 36.0],
        egui::Button::new(egui::RichText::new(label).size(18.0)),
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_interpolates_and_passes_through_endpoints() {
        let a = Mat4::from_translation(glam::Vec3::ZERO);
        let b = Mat4::from_rotation_translation(
            glam::Quat::from_rotation_y(1.0),
            glam::Vec3::new(2.0, 0.0, 0.0),
        );
        assert_eq!(blend(a, b, 1.0), b);
        let mid = blend(a, b, 0.5);
        let (_, r, t) = mid.to_scale_rotation_translation();
        assert!((t.x - 1.0).abs() < 1e-5);
        assert!((r.to_euler(glam::EulerRot::YXZ).0 - 0.5).abs() < 1e-4);
    }
}
