//! The editor's audio: sound previews in the Assets panel and the Audio
//! mixer panel.
//!
//! Both use one editor-only [`Preview`] (its own audio device, opened the
//! first time it is needed), so previews never touch a running game, which
//! is a separate process. The preview plays through a mixer set up from the
//! project's mixer settings (including unsaved edits), so bus volumes, mute
//! and solo are heard while you adjust them; its meters drive the panel.
//! The game itself reads the saved `project.json` `audio` section when it
//! starts.

use std::collections::HashMap;
use std::sync::Arc;

use copperframe_assets::AssetEntry;
use copperframe_assets::project::{AudioSettings, BusSettings};
use copperframe_audio::{AudioBus, Preview, PreviewState, Strip, peak_to_db};
use copperframe_core::AssetId;
use egui::{Color32, Rect, RichText, Sense, Stroke, vec2};
use kira::{AudioManagerSettings, DefaultBackend};

use crate::icons::{self, Icon};
use crate::session::EditorSession;
use crate::theme;

/// Meter colours.
const METER_OK: Color32 = Color32::from_rgb(110, 190, 110);
const METER_HOT: Color32 = Color32::from_rgb(230, 190, 60);
const METER_CLIP: Color32 = Color32::from_rgb(235, 80, 70);

/// Shown meter level and peak hold for one strip.
#[derive(Debug, Clone, Copy, Default)]
struct Level {
    level: f32,
    hold: f32,
    hold_time: f64,
}

/// The preview player and mixer meters, kept in `UiState`.
#[derive(Default)]
pub struct EditorAudio {
    preview: Option<Preview<DefaultBackend>>,
    /// Why the device could not be opened (shown instead of controls).
    pub error: Option<String>,
    levels: [Level; 5],
    /// Bus previews play on.
    pub bus: AudioBus,
    /// Seconds into the last previewed sound, for seeking before Play.
    last: Option<(AssetId, f64)>,
    /// Durations of sounds shown but not playing (decoding them every
    /// frame would be slow). Cleared when assets change.
    durations: HashMap<AssetId, f64>,
}

impl EditorAudio {
    /// The preview, opening the device the first time.
    fn device(&mut self, settings: AudioSettings) -> Option<&mut Preview<DefaultBackend>> {
        if self.preview.is_none() && self.error.is_none() {
            match Preview::open(AudioManagerSettings::default(), settings) {
                Ok(p) => self.preview = Some(p),
                Err(e) => {
                    tracing::warn!("sound preview unavailable: {e}");
                    self.error = Some(e.to_string());
                }
            }
        }
        self.preview.as_mut()
    }

    /// Keeps the preview mixer in step with the project's mixer settings
    /// and forgets finished sounds. Call once a frame.
    pub fn sync(&mut self, ctx: &egui::Context, settings: AudioSettings) {
        if let Some(p) = &mut self.preview {
            p.mixer().set_settings(settings);
            let was_playing = p.state().is_some();
            p.update();
            if was_playing && p.state().is_none() {
                // Played to the end: the next Play starts from the top.
                self.last = None;
            }
            if let Some(s) = p.state() {
                self.last = Some((s.asset, s.position));
                // Moving playhead and meters.
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
        }
        let now = ctx.input(|i| i.time);
        if let Some(p) = &self.preview {
            for (i, strip) in Strip::ALL.into_iter().enumerate() {
                let peak = p.mixer_ref().meter(strip).take_peak();
                let l = &mut self.levels[i];
                let dt = 1.0 / 30.0;
                // Fall about 20 dB a second on screen.
                l.level = peak.max(l.level * 10f32.powf(-20.0 * dt / 20.0));
                if peak >= l.hold || now - l.hold_time > 1.5 {
                    l.hold = peak;
                    l.hold_time = now;
                }
                if l.level > 0.001 {
                    ctx.request_repaint_after(std::time::Duration::from_millis(33));
                }
            }
        }
    }

    pub fn state(&self) -> Option<PreviewState> {
        self.preview.as_ref().and_then(Preview::state)
    }

    /// Plays `entry` (an audio asset) from `start` seconds.
    pub fn play(
        &mut self,
        session: &mut EditorSession,
        entry: &AssetEntry,
        settings: AudioSettings,
        start: f64,
    ) {
        let bytes =
            match copperframe_assets::import::audio::import_audio(&entry.source, &entry.settings) {
                Ok(a) => a.bytes,
                Err(e) => {
                    session.error(format!("cannot preview {}: {e}", entry.relative));
                    return;
                }
            };
        let bus = self.bus;
        let Some(p) = self.device(settings) else {
            return;
        };
        if let Err(e) = p.play(entry.id, Arc::clone(&bytes), bus, start) {
            session.error(format!("cannot preview {}: {e}", entry.relative));
        }
    }

    pub fn stop(&mut self) {
        if let Some(p) = &mut self.preview {
            p.stop();
        }
        self.last = None;
    }

    /// Stops the preview if it is playing an asset that is no longer in
    /// the project.
    /// Plays the sound if it is not the one playing, otherwise stops it
    /// (double-clicking a sound tile).
    pub fn toggle(
        &mut self,
        session: &mut EditorSession,
        entry: &AssetEntry,
        settings: AudioSettings,
    ) {
        if self.state().is_some_and(|s| s.asset == entry.id) {
            self.stop();
        } else {
            self.play(session, entry, settings, 0.0);
        }
    }

    pub fn forget_missing(&mut self, session: &EditorSession) {
        self.durations.clear();
        if let Some(s) = self.state()
            && session.db.get(s.asset).is_none()
        {
            self.stop();
        }
    }

    /// When the device could not be opened: says so, with Retry (for
    /// headphones plugged in later). Returns true while unavailable.
    pub fn unavailable(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(e) = &self.error else {
            return false;
        };
        ui.label(RichText::new("No sound output: previews are off").color(theme::TEXT_DIM))
            .on_hover_text(e);
        if ui
            .button("Retry")
            .on_hover_text("Look for a sound device again")
            .clicked()
        {
            self.error = None;
        }
        true
    }

    /// Play/pause, stop, position, volume and bus for the selected sound,
    /// in the Assets panel's details strip.
    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut EditorSession,
        entry: &AssetEntry,
        settings: AudioSettings,
    ) {
        if self.unavailable(ui) {
            return;
        }
        let state = self.state().filter(|s| s.asset == entry.id);
        let playing = state.is_some_and(|s| !s.paused);
        let icon = if playing { Icon::Pause } else { Icon::Play };
        let tip = if playing { "Pause" } else { "Play (preview)" };
        if icon_button(ui, icon, tip).clicked() {
            match (state, &mut self.preview) {
                (Some(s), Some(p)) if !s.paused => p.pause(),
                (Some(_), Some(p)) => p.resume(),
                _ => {
                    let start = match self.last {
                        Some((a, t)) if a == entry.id => t,
                        _ => 0.0,
                    };
                    self.play(session, entry, settings, start);
                }
            }
        }
        if icon_button(ui, Icon::Stop, "Stop").clicked() {
            self.stop();
        }
        let (position, duration) = match state {
            Some(s) => (s.position, s.duration),
            None => {
                let duration = *self.durations.entry(entry.id).or_insert_with(|| {
                    copperframe_assets::import::audio::import_audio(&entry.source, &entry.settings)
                        .map_or(0.0, |a| a.duration_secs)
                });
                let pos = match self.last {
                    Some((a, t)) if a == entry.id => t,
                    _ => 0.0,
                };
                (pos, duration)
            }
        };
        let mut seek = position;
        let r = ui.add(
            egui::Slider::new(&mut seek, 0.0..=duration.max(0.001))
                .show_value(false)
                .trailing_fill(true),
        );
        if r.changed() {
            match (state, &mut self.preview) {
                (Some(_), Some(p)) => p.seek(seek),
                _ => self.last = Some((entry.id, seek)),
            }
        }
        ui.label(
            RichText::new(format!("{} / {}", clock(position), clock(duration)))
                .monospace()
                .small(),
        );
        let mut volume = self.preview.as_ref().map_or(0.0, |p| p.volume_db());
        let r = ui
            .add(
                egui::DragValue::new(&mut volume)
                    .range(-60.0..=12.0)
                    .speed(0.2)
                    .suffix(" dB"),
            )
            .on_hover_text("Preview volume (on top of the mixer)");
        if r.changed()
            && let Some(p) = self.device(settings)
        {
            p.set_volume_db(volume);
        }
        let before = self.bus;
        egui::ComboBox::from_id_salt("preview-bus")
            .selected_text(format!("via {}", self.bus.label()))
            .show_ui(ui, |ui| {
                for bus in AudioBus::ALL {
                    ui.selectable_value(&mut self.bus, bus, bus.label());
                }
            })
            .response
            .on_hover_text("The mixer bus the preview plays through");
        if self.bus != before
            && let Some(s) = state
        {
            // Carry on from the same point on the new bus.
            self.play(session, entry, settings, s.position);
        }
    }

    /// The Audio mixer panel's strips. `settings` is the project's mixer
    /// (the Project settings draft); returns true if it changed.
    pub fn strips(&mut self, ui: &mut egui::Ui, settings: &mut AudioSettings) -> bool {
        let mut changed = false;
        let device = self.device(*settings).is_some();
        // Faders and meters grow with the panel.
        let height = (ui.available_height() - 110.0).clamp(60.0, 220.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().slider_width = height;
            for (i, strip) in Strip::ALL.into_iter().enumerate() {
                let level = self.levels[i];
                let clipped = self
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.mixer_ref().meter(strip).clipped());
                let solo = match (strip, &self.preview) {
                    (Strip::Bus(b), Some(p)) => Some(p.mixer_ref().solo(b)),
                    _ => None,
                };
                ui.group(|ui| {
                    ui.set_width(74.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(strip.label()).strong());
                        let bus: &mut BusSettings = match strip {
                            Strip::Master => &mut settings.master,
                            Strip::Bus(b) => settings.bus_mut(b),
                        };
                        ui.horizontal(|ui| {
                            if meter(ui, level, clipped, device, height).clicked()
                                && let Some(p) = &self.preview
                            {
                                p.mixer_ref().meter(strip).reset_clip();
                            }
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut bus.volume_db, BusSettings::DB_RANGE)
                                        .vertical()
                                        .show_value(false),
                                )
                                .on_hover_text("Bus volume")
                                .changed();
                        });
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut bus.volume_db)
                                    .range(BusSettings::DB_RANGE)
                                    .speed(0.2)
                                    .max_decimals(1)
                                    .suffix(" dB"),
                            )
                            .changed();
                        ui.horizontal(|ui| {
                            changed |= ui
                                .toggle_value(&mut bus.mute, "M")
                                .on_hover_text("Mute (saved with the project)")
                                .changed();
                            if let (Some(mut on), Strip::Bus(b)) = (solo, strip)
                                && ui
                                    .toggle_value(&mut on, "S")
                                    .on_hover_text(
                                        "Solo: hear only soloed buses in previews (editor only, \
                                         not saved)",
                                    )
                                    .changed()
                                && let Some(p) = &mut self.preview
                            {
                                p.mixer().set_solo(b, on);
                            }
                        });
                        if strip == Strip::Master {
                            changed |= ui
                                .checkbox(&mut settings.limiter, "Limiter")
                                .on_hover_text(
                                    "Pulls peaks down before they clip (a fast compressor at \
                                     -1 dBFS)",
                                )
                                .changed();
                        }
                    });
                });
            }
        });
        changed
    }
}

/// A vertical level meter: green, yellow above -6 dB, red at 0 dB, a
/// peak-hold line and a clip light on top (click it to reset).
fn meter(
    ui: &mut egui::Ui,
    level: Level,
    clipped: bool,
    device: bool,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(12.0, height), Sense::click());
    let p = ui.painter();
    let light = Rect::from_min_size(rect.min, vec2(rect.width(), 8.0));
    p.rect_filled(light, 2.0, if clipped { METER_CLIP } else { theme::WELL });
    let bar = Rect::from_min_max(rect.min + vec2(0.0, 11.0), rect.max);
    p.rect_filled(bar, 1.0, theme::WELL);
    let y_of = |peak: f32| {
        // -60..0 dB over the bar (and a little headroom up to +6).
        let db = peak_to_db(peak).clamp(-60.0, 6.0);
        let t = (db + 60.0) / 66.0;
        bar.bottom() - t * bar.height()
    };
    if device && level.level > 0.0 {
        let top = y_of(level.level);
        let db = peak_to_db(level.level);
        let color = if db >= 0.0 {
            METER_CLIP
        } else if db >= -6.0 {
            METER_HOT
        } else {
            METER_OK
        };
        p.rect_filled(
            Rect::from_min_max(egui::pos2(bar.left() + 1.0, top), bar.max - vec2(1.0, 0.0)),
            0.0,
            color,
        );
        if level.hold > 0.0 {
            let y = y_of(level.hold);
            p.hline(bar.x_range(), y, Stroke::new(1.0, theme::TEXT));
        }
    }
    // 0 dB mark.
    p.hline(bar.x_range(), y_of(1.0), Stroke::new(1.0, theme::TEXT_DIM));
    response.on_hover_text(if clipped {
        "Clipped: the signal reached 0 dBFS. Click to reset"
    } else {
        "Peak level (preview). The line marks 0 dBFS"
    })
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    let fill = if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    icons::paint(ui.painter(), rect.shrink(5.0), icon, theme::TEXT);
    response.on_hover_text(tip)
}

/// `m:ss.s`.
fn clock(secs: f64) -> String {
    let secs = secs.max(0.0);
    let m = (secs / 60.0).floor();
    format!("{}:{:04.1}", m as u64, secs - m * 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_minutes_and_tenths() {
        assert_eq!(clock(0.0), "0:00.0");
        assert_eq!(clock(3.25), "0:03.2");
        assert_eq!(clock(75.0), "1:15.0");
    }
}
