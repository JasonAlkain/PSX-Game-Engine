//! Application lifecycle for PSX Engine.
//!
//! A winit event loop that owns one window, a wgpu presenter, a fixed-step
//! simulation (fly camera + spinning crate) and per-frame rendering of the PS1
//! comparison scene with interpolation. Gameplay hooks, physics and audio
//! arrive in later milestones.

pub mod app;
pub mod comparison_scene;
pub mod fly_camera;
pub mod input;
pub mod logging;
pub mod style_controls;

pub use app::{AppConfig, run};
pub use logging::init_logging;
