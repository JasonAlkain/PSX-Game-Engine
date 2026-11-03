//! Shared engine types that have no window, GPU, or editor dependencies.
//!
//! Contains persistent IDs, simulation timing, atomic file writes and the
//! retro style settings (with their versioned JSON file format), and crash
//! and bug reports ([`report`]).

pub mod fsutil;
pub mod id;
pub mod report;
pub mod style;
pub mod time;

pub use id::{AssetId, EntityId, SceneId};
pub use style::{RetroStyle, Shading, Shadows, StyleError, StylePreset, TextureFilter, Textures};
pub use time::{FixedStep, StepPlan};
