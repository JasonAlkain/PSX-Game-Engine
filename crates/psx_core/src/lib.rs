//! Shared engine types that have no window, GPU, or editor dependencies.
//!
//! Milestone 1 only needs simulation timing. Stable IDs and project
//! configuration arrive with the scene format in Milestone 3.

pub mod time;

pub use time::{FixedStep, StepPlan};
