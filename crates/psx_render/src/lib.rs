//! wgpu renderer for PSX Engine.
//!
//! Milestone 1 scope: GPU device setup, surface presentation with recoverable
//! error handling, a perspective camera, and a forward pass that draws a
//! textured, vertex-lit mesh. Retro passes (internal resolution, snapping,
//! affine UVs, dithering) arrive in Milestone 2.
//!
//! This crate does not depend on a windowing library. Callers pass anything
//! that converts into [`wgpu::SurfaceTarget`] (e.g. `Arc<winit::window::Window>`).

pub mod camera;
pub mod context;
pub mod mesh;
pub mod presenter;
pub mod renderer;
pub mod texture;

pub use camera::Camera;
pub use context::{GpuContext, GpuError};
pub use mesh::{MeshData, MeshError, Vertex};
pub use presenter::{FrameAcquire, Presenter};
pub use renderer::{DepthTarget, FrameUniforms, GpuMesh, GpuTexture, Lighting, MeshRenderer};
pub use texture::{TextureData, TextureError};

/// Re-exported so downstream crates use the exact wgpu version this crate was built with.
pub use wgpu;
