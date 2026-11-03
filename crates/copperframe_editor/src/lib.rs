//! Copperframe Engine scene editor.
//!
//! - [`session`]: project/scene state, undo history, selection, log (no UI).
//! - [`viewport`]: screen ↔ world conversion for picking and gizmos, and the
//!   editor camera.
//! - [`ui`]: egui panels.
//! - [`gameplay`]: collider/audio/behaviour inspectors, Play/Stop as a
//!   child process, and export.
//! - [`visuals`]: light, blob shadow and animator inspectors.
//! - [`app`]: window, event loop and rendering.

pub mod app;
pub mod gameplay;
pub mod session;
pub mod ui;
pub mod viewport;
pub mod visuals;

pub use app::{EditorConfig, run};
pub use session::{EditorSession, SaveOutcome};
