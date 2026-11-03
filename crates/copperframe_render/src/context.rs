//! Adapter and device creation.

use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no compatible graphics adapter found: {0}")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    #[error("graphics device creation failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("could not create a window surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("the graphics adapter cannot present to this window")]
    SurfaceUnsupported,
}

/// Instance, adapter, device and queue shared by the renderer.
#[derive(Debug)]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Requests an adapter (compatible with `surface` if given) and a device
    /// using only WebGPU default limits and no optional features, so any
    /// Vulkan, DX12, Metal or GL-ES 3 class GPU qualifies.
    ///
    /// Backends and validation can be overridden with the standard `WGPU_BACKEND`,
    /// `WGPU_VALIDATION` and `WGPU_DEBUG` environment variables.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::from_env()
                    .unwrap_or(wgpu::PowerPreference::HighPerformance),
                force_fallback_adapter: false,
                compatible_surface: surface,
                apply_limit_buckets: false,
            })
            .await?;

        let info = adapter.get_info();
        tracing::info!(
            adapter = %info.name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            driver = %info.driver,
            driver_info = %info.driver_info,
            "selected graphics adapter"
        );
        copperframe_core::report::set_context(
            "graphics",
            format!(
                "{} ({:?}, {:?}, driver {} {})",
                info.name, info.backend, info.device_type, info.driver, info.driver_info
            ),
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("copperframe device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await?;

        // Anything not caught by an error scope ends up here. Log instead of
        // the default handler's panic so the application can report it.
        device.on_uncaptured_error(Arc::new(|error| {
            tracing::error!("wgpu error: {error:?}");
        }));

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    /// Context without a surface, for offscreen rendering and tests.
    pub async fn headless() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::new(instance, None).await
    }
}
