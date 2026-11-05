//! Copperframe Engine scene editor.
//!
//! - [`hub`]: the project hub (start screen): recent projects, open, new
//!   project from a template.
//! - [`watch`]: picks up files other programs change in `assets/` and
//!   `scenes/`.
//! - [`session`]: project/scene state, undo history, selection, log (no UI).
//! - [`viewport`]: screen ↔ world conversion for picking and gizmos, and the
//!   editor camera.
//! - [`ui`]: menus, inspector, scene settings and dialogs.
//! - [`project_settings`]: the Project Settings panel (project.json and
//!   input.json).
//! - [`dock`]: the dockable panel layout and the Window menu.
//! - [`toolbar`]: transform tools, snapping and view options.
//! - [`gizmo`]: transform tool maths and snapping.
//! - [`snap_settings`]: the Grid and snapping window.
//! - [`viewport_panel`]: the 3D panel: picking, camera, gizmos.
//! - [`assets_panel`]: the asset browser.
//! - [`theme`], [`icons`]: the editor's colours and drawn icons.
//! - [`gameplay`]: collider/audio/behaviour inspectors, Play/Stop as a
//!   child process, and the Build menu's jobs (validate, export, tests,
//!   benchmark).
//! - [`reporting`]: crash prompt and "Report a bug" (email drafts only).
//! - [`visuals`]: light, blob shadow and animator inspectors.
//! - [`app`]: window, event loop and rendering.

pub mod app;
pub mod assets_panel;
pub mod components;
pub mod dock;
pub mod gameplay;
pub mod gizmo;
pub mod hub;
pub mod icons;
pub mod project_settings;
pub mod reporting;
pub mod session;
pub mod snap_settings;
pub mod theme;
pub mod toolbar;
pub mod ui;
pub mod viewport;
pub mod viewport_panel;
pub mod visuals;
pub mod watch;

pub use app::{EditorConfig, run};
pub use session::{EditorSession, SaveOutcome};
