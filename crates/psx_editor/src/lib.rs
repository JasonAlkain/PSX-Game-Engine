//! PSX Engine scene editor.
//!
//! - [`session`]: project/scene state, undo history, selection, log (no UI).
//! - [`viewport`]: screen ↔ world conversion for picking and gizmos, and the
//!   editor camera.
//! - [`ui`]: egui panels.
//! - [`app`]: window, event loop and rendering.

pub mod app;
pub mod session;
pub mod ui;
pub mod viewport;

pub use app::{EditorConfig, run};
pub use session::{EditorSession, SaveOutcome};
