//! Assets and projects for PSX Engine.
//!
//! - [`meta`]: persistent asset IDs in `<source>.meta.json` sidecars.
//! - [`import`]: static glTF/GLB and PNG/JPEG importers producing CPU data.
//! - [`project`]: project layout (`project.json`, `assets/`, `scenes/`),
//!   the asset database, the editor lock, and project validation shared by
//!   the editor and the CLI.
//!
//! No window or GPU dependencies.

pub mod fixtures;
pub mod import;
pub mod meta;
pub mod project;

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
