//! Application lifecycle for PSX Engine.
//!
//! Milestone 1: a winit event loop that owns one window, a wgpu presenter, a
//! fixed-step simulation (fly camera + spinning test mesh) and per-frame
//! rendering with interpolation. Gameplay hooks, physics and audio arrive in
//! later milestones.

pub mod app;
pub mod fly_camera;
pub mod input;
pub mod logging;

pub use app::{AppConfig, run};
pub use logging::init_logging;
