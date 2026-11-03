//! Copperframe branding shared by the editor, player and sandbox: the window
//! icon and the launch splash.
//!
//! The source artwork lives in `assets/branding/`; the copies embedded here
//! are reduced in size (`branding/icon_64.png`, `branding/splash.png`).

use std::time::{Duration, Instant};

const ICON_PNG: &[u8] = include_bytes!("../branding/icon_64.png");
const SPLASH_PNG: &[u8] = include_bytes!("../branding/splash.png");

/// Background behind the splash image, matching the header artwork.
const SPLASH_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(18, 19, 21);
const SPLASH_HOLD: Duration = Duration::from_millis(2000);
const SPLASH_FADE: Duration = Duration::from_millis(400);

fn decode(png: &[u8]) -> Option<image::RgbaImage> {
    match image::load_from_memory_with_format(png, image::ImageFormat::Png) {
        Ok(img) => Some(img.to_rgba8()),
        Err(e) => {
            tracing::warn!("embedded branding image did not decode: {e}");
            None
        }
    }
}

/// The Copperframe window icon, or `None` if it could not be built. Windows
/// and X11 show it; macOS and Wayland ignore window icons.
pub fn window_icon() -> Option<winit::window::Icon> {
    let img = decode(ICON_PNG)?;
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h)
        .map_err(|e| tracing::warn!("window icon rejected: {e}"))
        .ok()
}

/// The Copperframe header artwork as an egui texture (the project hub's
/// banner). `None` if it could not be decoded.
pub fn header_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = decode(SPLASH_PNG)?;
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("copperframe-header", color, egui::TextureOptions::LINEAR))
}

/// The launch splash: the Copperframe header over the whole window for a
/// moment, then a fade. Any key press or click skips it, and the input that
/// skips it does not reach the UI underneath.
pub struct Splash {
    texture: Option<egui::TextureHandle>,
    started: Option<Instant>,
    done: bool,
}

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl Splash {
    pub fn new() -> Self {
        Self {
            texture: None,
            started: None,
            done: false,
        }
    }

    /// A splash that never shows (for runs that skip it).
    pub fn disabled() -> Self {
        Self {
            done: true,
            ..Self::new()
        }
    }

    pub fn is_active(&self) -> bool {
        !self.done
    }

    /// Opacity at `elapsed` since the first frame: 1 while holding, then a
    /// linear fade to 0. `None` once finished.
    fn opacity(elapsed: Duration) -> Option<f32> {
        if elapsed < SPLASH_HOLD {
            Some(1.0)
        } else if elapsed < SPLASH_HOLD + SPLASH_FADE {
            Some(1.0 - (elapsed - SPLASH_HOLD).as_secs_f32() / SPLASH_FADE.as_secs_f32())
        } else {
            None
        }
    }

    /// Draws the splash above everything else. Call once per frame inside
    /// the egui pass, after the rest of the UI.
    pub fn show(&mut self, ctx: &egui::Context) {
        if self.done {
            return;
        }
        let started = *self.started.get_or_insert_with(Instant::now);
        let skipped = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::PointerButton { pressed: true, .. }
                )
            })
        });
        let Some(opacity) = Self::opacity(started.elapsed()).filter(|_| !skipped) else {
            self.done = true;
            self.texture = None;
            return;
        };
        if self.texture.is_none() {
            let Some(img) = decode(SPLASH_PNG) else {
                self.done = true;
                return;
            };
            let size = [img.width() as usize, img.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            self.texture =
                Some(ctx.load_texture("copperframe-splash", color, egui::TextureOptions::LINEAR));
        }
        let Some(texture) = &self.texture else {
            return;
        };

        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("copperframe-splash"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                // Swallow clicks so the UI underneath does not react.
                ui.allocate_rect(screen, egui::Sense::click());
                let tint = egui::Color32::WHITE.gamma_multiply(opacity);
                ui.painter()
                    .rect_filled(screen, 0.0, SPLASH_BACKGROUND.gamma_multiply(opacity));
                let tex = texture.size_vec2();
                let scale = (screen.width() * 0.85 / tex.x)
                    .min(screen.height() * 0.85 / tex.y)
                    .min(1.0);
                let rect = egui::Rect::from_center_size(screen.center(), tex * scale);
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    tint,
                );
            });
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_images_decode() {
        assert!(window_icon().is_some());
        let splash = decode(SPLASH_PNG).unwrap();
        assert!(splash.width() > splash.height());
    }

    #[test]
    fn splash_holds_then_fades_then_ends() {
        assert_eq!(Splash::opacity(Duration::ZERO), Some(1.0));
        let mid = Splash::opacity(SPLASH_HOLD + SPLASH_FADE / 2).unwrap();
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        assert_eq!(Splash::opacity(SPLASH_HOLD + SPLASH_FADE), None);
        assert!(!Splash::disabled().is_active());
    }
}
