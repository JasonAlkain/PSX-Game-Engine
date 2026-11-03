//! Shared engine types that have no window, GPU, or editor dependencies.
//!
//! Contains simulation timing and the retro style settings (with their
//! versioned JSON file format). Stable IDs and project configuration arrive
//! with the scene format in Milestone 3.

pub mod style;
pub mod time;

pub use style::{RetroStyle, StyleError, StylePreset};
pub use time::{FixedStep, StepPlan};
