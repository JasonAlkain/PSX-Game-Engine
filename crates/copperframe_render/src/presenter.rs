//! Window surface configuration and frame acquisition.

use crate::GpuContext;
use crate::context::GpuError;

/// Outcome of trying to acquire the next swapchain image.
#[derive(Debug)]
pub enum FrameAcquire {
    /// Render into this texture and call `present()`.
    Ready(wgpu::SurfaceTexture),
    /// Nothing to draw this frame (minimized, occluded, timed out, or the
    /// surface was reconfigured). Try again next frame.
    Skip,
    /// The surface was lost and must be recreated from the window.
    Lost,
}

/// Owns a configured `wgpu::Surface`.
#[derive(Debug)]
pub struct Presenter {
    /// `None` only between dropping a lost surface and creating its
    /// replacement (see [`Presenter::recreate_surface`]).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    /// Set when the last acquired frame was suboptimal. The surface is
    /// reconfigured before the next acquire, never while a frame is held.
    reconfigure_pending: bool,
    /// Set after a `Lost` result has been answered with a reconfigure, so a
    /// second consecutive `Lost` escalates to surface recreation.
    lost_reconfigure_tried: bool,
}

impl Presenter {
    /// Configures `surface` for presentation at `width`×`height` physical pixels.
    ///
    /// An sRGB format is preferred so the composite shader's linear output is
    /// encoded by the hardware; with any other format the composite shader
    /// encodes instead (see `shaders/composite.wgsl`). A zero size is allowed; the surface stays unconfigured until a
    /// non-zero `resize`.
    pub fn new(
        gpu: &GpuContext,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, GpuError> {
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or(GpuError::SurfaceUnsupported)?;
        if !format.is_srgb() {
            tracing::info!(
                ?format,
                "no sRGB surface format; the composite pass writes sRGB-encoded values itself"
            );
        }
        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        // Overlays such as the editor UI expect a non-sRGB target (they
        // blend in gamma space); offer that view when the backend allows it.
        let overlay_view = (format.is_srgb()
            && gpu
                .adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS))
        .then(|| format.remove_srgb_suffix());
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: overlay_view.into_iter().collect(),
        };
        tracing::info!(?format, ?alpha_mode, "surface format selected");
        let presenter = Self {
            surface: Some(surface),
            config,
            reconfigure_pending: false,
            lost_reconfigure_tried: false,
        };
        presenter.configure(gpu);
        Ok(presenter)
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Format for overlay passes that blend in gamma space (egui): the
    /// non-sRGB variant of [`Presenter::format`] when the surface allows
    /// that view, otherwise the surface format itself.
    pub fn overlay_format(&self) -> wgpu::TextureFormat {
        self.config
            .view_formats
            .first()
            .copied()
            .unwrap_or(self.config.format)
    }

    /// Current size in physical pixels.
    /// Switches between vsync (`AutoVsync`) and lowest latency
    /// (`AutoNoVsync`, which falls back to vsync where unsupported).
    pub fn set_vsync(&mut self, gpu: &GpuContext, vsync: bool) {
        let mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.configure(gpu);
        }
    }

    pub fn vsync(&self) -> bool {
        self.config.present_mode == wgpu::PresentMode::AutoVsync
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    fn configure(&self, gpu: &GpuContext) {
        if let Some(surface) = &self.surface
            && self.config.width > 0
            && self.config.height > 0
        {
            surface.configure(&gpu.device, &self.config);
        }
    }

    /// Reconfigures for a new size. Zero sizes (minimized windows) are
    /// recorded but not applied; acquisition is skipped until the size is valid.
    pub fn resize(&mut self, gpu: &GpuContext, width: u32, height: u32) {
        if (width, height) == self.size() {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.configure(gpu);
    }

    /// Replaces a lost surface and configures it with the current settings.
    ///
    /// The old surface is dropped *before* `create` runs: Vulkan allows only
    /// one surface/swapchain per native window, and creating the replacement
    /// first fails (observed as a driver panic on Mesa lavapipe/X11). If
    /// `create` fails, frames are skipped and the next call can retry.
    pub fn recreate_surface<E>(
        &mut self,
        gpu: &GpuContext,
        create: impl FnOnce() -> Result<wgpu::Surface<'static>, E>,
    ) -> Result<(), E> {
        self.surface = None;
        self.surface = Some(create()?);
        self.reconfigure_pending = false;
        self.lost_reconfigure_tried = false;
        self.configure(gpu);
        Ok(())
    }

    pub fn acquire(&mut self, gpu: &GpuContext) -> FrameAcquire {
        if self.config.width == 0 || self.config.height == 0 {
            return FrameAcquire::Skip;
        }
        if std::mem::take(&mut self.reconfigure_pending) {
            self.configure(gpu);
        }
        let Some(surface) = &self.surface else {
            return FrameAcquire::Lost;
        };
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => {
                self.lost_reconfigure_tried = false;
                FrameAcquire::Ready(frame)
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.lost_reconfigure_tried = false;
                tracing::debug!("surface suboptimal; reconfiguring before the next frame");
                self.reconfigure_pending = true;
                FrameAcquire::Ready(frame)
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                FrameAcquire::Skip
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                tracing::debug!("surface outdated; reconfiguring");
                self.configure(gpu);
                FrameAcquire::Skip
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // Try the cheap fix first: rebuild the swapchain on the
                // existing surface. Only if that does not help is the caller
                // asked to recreate the surface itself.
                if std::mem::replace(&mut self.lost_reconfigure_tried, true) {
                    FrameAcquire::Lost
                } else {
                    tracing::warn!("surface lost; reconfiguring swapchain");
                    self.configure(gpu);
                    FrameAcquire::Skip
                }
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::error!("validation error while acquiring a surface texture");
                FrameAcquire::Skip
            }
        }
    }
}
