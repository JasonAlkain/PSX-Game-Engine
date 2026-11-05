//! Assets and projects for Copperframe Engine.
//!
//! - [`meta`]: persistent asset IDs in `<source>.meta.json` sidecars.
//! - [`import`]: glTF/GLB (static and skinned, with animation clips),
//!   PNG/JPEG and WAV/Ogg importers producing CPU data.
//! - [`anim`]: skeletons, clips, pose sampling and blending.
//! - [`project`]: project layout (`project.json`, `assets/`, `scenes/`),
//!   the asset database, the editor lock, and project validation shared by
//!   the editor and the CLI.
//! - [`project_settings`]: the project-wide settings in `project.json`.
//! - [`input`]: the project's default key bindings (`input.json`).
//! - [`game`]: finding, building and running a project's game executable.
//! - [`export`]: host-platform export of a game and its data.
//! - [`template`]: new projects from templates, and their Rust game crate.
//!
//! No window or GPU dependencies.

pub mod anim;
pub mod export;
pub mod fixtures;
pub mod game;
pub mod import;
pub mod input;
pub mod meta;
pub mod project;
pub mod project_settings;
pub mod template;

pub use import::audio::{AudioFormat, ImportedAudio, import_audio};
pub use import::gltf::import_gltf;
pub use import::texture::import_texture;
pub use import::{
    CpuMesh, CpuTexture, IMPORTER_VERSION, ImportError, ImportedMaterial, ImportedModel,
    MeshVertex, ModelPart,
};
pub use meta::{AssetKind, AssetMeta, ImportSettings};
pub use project::{
    AssetDatabase, AssetEntry, Diagnostic, EditorLock, ImportedAsset, Project, ProjectError,
    Severity, ValidationReport, validate_project,
};
