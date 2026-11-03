//! wgpu renderer for PSX Engine.
//!
//! Renders the world into an internal-resolution target (PS1 style: vertex
//! snapping, affine UVs, nearest-sampled textures, vertex lighting, fog), then
//! composites it into the window with integer upscaling or aspect-correct
//! fitting, ordered dithering and colour reduction. See [`retro`] for the pass
//! structure and [`layout`] for the fit policy.
//!
//! This crate does not depend on a windowing library. Callers pass anything
//! that converts into [`wgpu::SurfaceTarget`] (e.g. `Arc<winit::window::Window>`).

pub mod camera;
pub mod context;
pub mod layout;
pub mod material;
pub mod mesh;
pub mod presenter;
pub mod retro;
pub mod texture;

pub use camera::Camera;
pub use context::{GpuContext, GpuError};
pub use layout::{Placement, Rect, ViewLayout};
pub use material::{AlphaMode, Material};
pub use mesh::{MeshData, MeshError, Vertex};
pub use presenter::{FrameAcquire, Presenter};
pub use retro::{
    CompositeSlot, DrawItem, GpuMesh, GpuTexture, Lighting, RenderError, RenderView, RetroRenderer,
};
pub use texture::{TextureData, TextureError};

/// Re-exported so downstream crates use the exact wgpu version this crate was built with.
pub use wgpu;
