//! Copperframe Engine scene editor.
//!
//! - [`hub`]: the project hub (start screen): recent projects, open, new
//!   project from a template.
//! - [`watch`]: picks up files other programs change in `assets/` and
//!   `scenes/`.
//! - [`session`]: project/scene state, undo history, selection, log (no UI).
//! - [`viewport`]: screen ↔ world conversion for picking and gizmos, and the
//!   editor camera.
//! - [`ui`]: egui panels.
//! - [`gameplay`]: collider/audio/behaviour inspectors, Play/Stop as a
//!   child process, and the Build menu's jobs (validate, export, tests,
//!   benchmark).
//! - [`reporting`]: crash prompt and "Report a bug" (email drafts only).
//! - [`visuals`]: light, blob shadow and animator inspectors.
//! - [`app`]: window, event loop and rendering.

pub mod app;
pub mod gameplay;
pub mod hub;
pub mod reporting;
pub mod session;
pub mod ui;
pub mod viewport;
pub mod visuals;
pub mod watch;

pub use app::{EditorConfig, run};
pub use session::{EditorSession, SaveOutcome};
