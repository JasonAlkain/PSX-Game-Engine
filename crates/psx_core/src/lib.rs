//! Shared engine types that have no window, GPU, or editor dependencies.
//!
//! Contains persistent IDs, simulation timing, atomic file writes and the
//! retro style settings (with their versioned JSON file format).

pub mod fsutil;
pub mod id;
pub mod style;
pub mod time;

pub use id::{AssetId, EntityId, SceneId};
pub use style::{RetroStyle, StyleError, StylePreset};
pub use time::{FixedStep, StepPlan};
