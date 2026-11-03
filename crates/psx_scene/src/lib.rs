//! Authored scene model for PSX Engine.
//!
//! - [`Scene`]: entities with persistent [`psx_core::EntityId`]s, a parent
//!   hierarchy with local transforms, mesh, collider, audio and behaviour
//!   components ([`components`]) and scene settings.
//! - [`file`]: the versioned `psx_scene` JSON format (atomic saves).
//! - [`command`]: every authoring edit as a validated [`Command`] that
//!   produces an exactly reversible [`Change`].
//! - [`History`]: undo/redo built on those changes.
//! - [`batch`]: versioned command batches applied all-or-nothing (CLI).
//!
//! No window, GPU or editor dependencies: the editor and the CLI share this
//! crate so both use the same validation and serialisation rules.

pub mod batch;
pub mod command;
pub mod components;
pub mod file;
pub mod history;
pub mod scene;
pub mod transform;

pub use batch::{BatchError, BatchReport, CommandBatch, apply_batch, parse_batch};
pub use command::{Change, Command, CommandError, apply};
pub use components::{
    AudioBus, AudioSourceComponent, BehaviorComponent, BodyKind, ColliderComponent, ColliderShape,
};
pub use file::{SceneFileError, load_scene, parse_scene, save_scene, scene_to_json};
pub use history::History;
pub use scene::{
    AlphaSetting, CameraStart, Entity, MaterialOverride, MeshComponent, MeshSource, PrimitiveShape,
    Scene, SceneLighting, SceneProblem, SceneSettings,
};
pub use transform::{Transform, euler_deg_to_quat, quat_to_euler_deg};
