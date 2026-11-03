//! Audio integration (kira).
//!
//! Two buses (music and effects) under a master volume, one listener (the
//! active camera), spatial emitters that follow entities, one-shot sounds,
//! looping and fades. If no audio device can be opened the game keeps
//! running silently: every call still updates the logical state (what is
//! playing), so gameplay and headless tests behave the same.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use glam::{Quat, Vec3};
use kira::backend::Backend;
use kira::listener::ListenerHandle;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{SpatialTrackBuilder, SpatialTrackHandle, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Tween};
use psx_core::AssetId;
use psx_scene::{AudioBus, AudioSourceComponent};

/// Linear volume (0..=1) to decibels.
pub fn linear_to_db(v: f32) -> f32 {
    if v <= 0.001 {
        Decibels::SILENCE.0
    } else {
        (20.0 * v.log10()).max(Decibels::SILENCE.0)
    }
}

fn tween(secs: f32) -> Tween {
    Tween {
        duration: Duration::from_secs_f32(secs.max(0.0)),
        ..Default::default()
    }
}

/// Player-facing volumes, each 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volumes {
    pub master: f32,
    pub music: f32,
    pub effects: f32,
}

impl Default for Volumes {
    fn default() -> Self {
        Self {
            master: 0.8,
            music: 0.7,
            effects: 1.0,
        }
    }
}

struct Device<B: Backend> {
    manager: AudioManager<B>,
    music: TrackHandle,
    effects: TrackHandle,
    listener: ListenerHandle,
}

struct Emitter {
    source: AudioSourceComponent,
    position: Vec3,
    track: Option<SpatialTrackHandle>,
    sound: Option<StaticSoundHandle>,
    /// Logical state: started and not stopped (and, for one-shots, not yet
    /// past its length).
    playing: bool,
    remaining: Option<f64>,
}

/// Something the audio system did, for logs and headless reports.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent<E> {
    Started { emitter: E, sound: AssetId },
    Stopped { emitter: E },
    OneShot { sound: AssetId },
}

pub struct Audio<E: Copy + Eq + std::hash::Hash, B: Backend = DefaultBackend> {
    device: Option<Device<B>>,
    sounds: HashMap<AssetId, StaticSoundData>,
    emitters: HashMap<E, Emitter>,
    volumes: Volumes,
    events: Vec<AudioEvent<E>>,
}

impl<E: Copy + Eq + std::hash::Hash> Audio<E, DefaultBackend> {
    /// Opens the default output device. Without one, returns a silent
    /// instance and the reason.
    pub fn open_default() -> (Self, Option<String>) {
        Self::open(AudioManagerSettings::default())
    }
}

impl<E: Copy + Eq + std::hash::Hash, B: Backend> Audio<E, B>
where
    B::Error: std::fmt::Debug,
{
    pub fn open(settings: AudioManagerSettings<B>) -> (Self, Option<String>) {
        let device = AudioManager::<B>::new(settings)
            .map_err(|e| format!("no audio output ({e:?}); continuing without sound"))
            .and_then(|mut manager| {
                let music = manager
                    .add_sub_track(TrackBuilder::new())
                    .map_err(|e| e.to_string())?;
                let effects = manager
                    .add_sub_track(TrackBuilder::new())
                    .map_err(|e| e.to_string())?;
                let listener = manager
                    .add_listener(Vec3::ZERO, Quat::IDENTITY)
                    .map_err(|e| e.to_string())?;
                Ok(Device {
                    manager,
                    music,
                    effects,
                    listener,
                })
            });
        let (device, warning) = match device {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e)),
        };
        let mut audio = Self {
            device,
            sounds: HashMap::new(),
            emitters: HashMap::new(),
            volumes: Volumes::default(),
            events: Vec::new(),
        };
        audio.set_volumes(Volumes::default());
        (audio, warning)
    }

    /// A silent instance (headless simulation).
    pub fn silent() -> Self {
        Self {
            device: None,
            sounds: HashMap::new(),
            emitters: HashMap::new(),
            volumes: Volumes::default(),
            events: Vec::new(),
        }
    }

    pub fn has_device(&self) -> bool {
        self.device.is_some()
    }

    pub fn backend_mut(&mut self) -> Option<&mut B> {
        self.device.as_mut().map(|d| d.manager.backend_mut())
    }

    /// Decodes a WAV/Ogg file for later playback.
    pub fn load(&mut self, asset: AssetId, bytes: Arc<[u8]>) -> Result<(), String> {
        let data = StaticSoundData::from_cursor(Cursor::new(bytes))
            .map_err(|e| format!("could not decode sound: {e}"))?;
        self.sounds.insert(asset, data);
        Ok(())
    }

    pub fn is_loaded(&self, asset: AssetId) -> bool {
        self.sounds.contains_key(&asset)
    }

    pub fn set_volumes(&mut self, v: Volumes) {
        self.volumes = v;
        if let Some(d) = &mut self.device {
            d.manager
                .main_track()
                .set_volume(linear_to_db(v.master), tween(0.05));
            d.music.set_volume(linear_to_db(v.music), tween(0.05));
            d.effects.set_volume(linear_to_db(v.effects), tween(0.05));
        }
    }

    pub fn volumes(&self) -> Volumes {
        self.volumes
    }

    /// Moves the listener (the active camera). `rotation` maps the
    /// listener's local axes (−Z forward, +Y up) to world.
    pub fn set_listener(&mut self, position: Vec3, rotation: Quat) {
        if let Some(d) = &mut self.device {
            d.listener.set_position(position, Tween::default());
            d.listener.set_orientation(rotation, Tween::default());
        }
    }

    /// Registers an entity's audio source; starts it if `autoplay`.
    pub fn add_emitter(&mut self, key: E, source: AudioSourceComponent, position: Vec3) {
        let mut emitter = Emitter {
            source,
            position,
            track: None,
            sound: None,
            playing: false,
            remaining: None,
        };
        if source.spatial
            && let Some(d) = &mut self.device
        {
            let builder = SpatialTrackBuilder::new()
                .distances((source.min_distance, source.max_distance))
                .persist_until_sounds_finish(false);
            let bus = match source.bus {
                AudioBus::Music => &mut d.music,
                AudioBus::Effects => &mut d.effects,
            };
            match bus.add_spatial_sub_track(&d.listener, position, builder) {
                Ok(track) => emitter.track = Some(track),
                Err(e) => tracing::warn!("audio emitter limit reached: {e}"),
            }
        }
        self.emitters.insert(key, emitter);
        if source.autoplay {
            self.play(key, 0.0);
        }
    }

    /// Starts (or restarts) an emitter's sound, fading in over `fade_secs`.
    pub fn play(&mut self, key: E, fade_secs: f32) {
        let Some(emitter) = self.emitters.get_mut(&key) else {
            return;
        };
        let source = emitter.source;
        let Some(data) = self.sounds.get(&source.sound) else {
            tracing::warn!("sound {} is not loaded; emitter stays silent", source.sound);
            return;
        };
        if let Some(old) = &mut emitter.sound {
            old.stop(tween(0.02));
        }
        let mut data = data.volume(source.volume_db);
        if source.looping {
            data = data.loop_region(..);
        }
        if fade_secs > 0.0 {
            data = data.fade_in_tween(tween(fade_secs));
        }
        emitter.remaining = (!source.looping).then(|| data.duration().as_secs_f64());
        emitter.sound = match (&mut emitter.track, &mut self.device) {
            (Some(track), _) => track.play(data).ok(),
            (None, Some(d)) => match source.bus {
                AudioBus::Music => d.music.play(data).ok(),
                AudioBus::Effects => d.effects.play(data).ok(),
            },
            (None, None) => None,
        };
        emitter.playing = true;
        self.events.push(AudioEvent::Started {
            emitter: key,
            sound: source.sound,
        });
    }

    /// Stops an emitter, fading out over `fade_secs`.
    pub fn stop(&mut self, key: E, fade_secs: f32) {
        if let Some(e) = self.emitters.get_mut(&key) {
            if let Some(s) = &mut e.sound {
                s.stop(tween(fade_secs));
            }
            if e.playing {
                self.events.push(AudioEvent::Stopped { emitter: key });
            }
            e.playing = false;
            e.remaining = None;
        }
    }

    pub fn set_emitter_position(&mut self, key: E, position: Vec3) {
        if let Some(e) = self.emitters.get_mut(&key)
            && e.position != position
        {
            e.position = position;
            if let Some(t) = &mut e.track {
                t.set_position(position, Tween::default());
            }
        }
    }

    pub fn remove_emitter(&mut self, key: E) {
        self.stop(key, 0.05);
        self.emitters.remove(&key);
    }

    /// Plays a sound once on a bus (not attached to an entity).
    pub fn play_one_shot(&mut self, sound: AssetId, bus: AudioBus, volume_db: f32) {
        let Some(data) = self.sounds.get(&sound) else {
            tracing::warn!("sound {sound} is not loaded");
            return;
        };
        if let Some(d) = &mut self.device {
            let data = data.volume(volume_db);
            let result = match bus {
                AudioBus::Music => d.music.play(data).map(|_| ()),
                AudioBus::Effects => d.effects.play(data).map(|_| ()),
            };
            if let Err(e) = result {
                tracing::warn!("could not play sound: {e}");
            }
        }
        self.events.push(AudioEvent::OneShot { sound });
    }

    /// Advances logical playback by `dt` seconds (ends finished one-shots).
    pub fn update(&mut self, dt: f64) {
        for (key, e) in &mut self.emitters {
            if !e.playing {
                continue;
            }
            let device_done = e
                .sound
                .as_ref()
                .is_some_and(|s| matches!(s.state(), PlaybackState::Stopped));
            if let Some(r) = &mut e.remaining {
                *r -= dt;
            }
            let time_done = e.sound.is_none() && e.remaining.is_some_and(|r| r <= 0.0);
            if device_done || time_done {
                e.playing = false;
                self.events.push(AudioEvent::Stopped { emitter: *key });
            }
        }
    }

    pub fn is_playing(&self, key: E) -> bool {
        self.emitters.get(&key).is_some_and(|e| e.playing)
    }

    /// Device-level state of an emitter's current sound, if a device exists.
    pub fn device_state(&self, key: E) -> Option<PlaybackState> {
        self.emitters
            .get(&key)
            .and_then(|e| e.sound.as_ref())
            .map(|s| s.state())
    }

    /// Stops and forgets every emitter (scene unload). Loaded sounds stay
    /// cached.
    pub fn clear_emitters(&mut self) {
        let keys: Vec<E> = self.emitters.keys().copied().collect();
        for k in keys {
            self.remove_emitter(k);
        }
    }

    pub fn emitter_count(&self) -> usize {
        self.emitters.len()
    }

    pub fn take_events(&mut self) -> Vec<AudioEvent<E>> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::backend::mock::{MockBackend, MockBackendSettings};

    fn tone() -> Arc<[u8]> {
        let samples: Vec<f32> = (0..4410).map(|i| (i as f32 * 0.1).sin()).collect();
        psx_assets::import::audio::wav_bytes(44100, &samples).into()
    }

    fn mock() -> Audio<u32, MockBackend> {
        let (audio, warning) = Audio::<u32, MockBackend>::open(AudioManagerSettings {
            backend_settings: MockBackendSettings::default(),
            ..Default::default()
        });
        assert!(warning.is_none());
        audio
    }

    #[test]
    fn spatial_emitters_play_loop_and_stop() {
        let mut audio = mock();
        let sound = AssetId::new();
        audio.load(sound, tone()).unwrap();
        let mut source = AudioSourceComponent::new(sound);
        source.looping = true;
        source.autoplay = true;
        audio.add_emitter(7, source, Vec3::new(3.0, 0.0, 0.0));
        audio.set_listener(Vec3::ZERO, Quat::IDENTITY);
        if let Some(b) = audio.backend_mut() {
            b.on_start_processing();
            for _ in 0..8 {
                b.process();
            }
        }
        assert!(audio.is_playing(7));
        assert_eq!(audio.device_state(7), Some(PlaybackState::Playing));
        // Looping sounds outlive their length.
        audio.update(5.0);
        assert!(audio.is_playing(7));
        audio.stop(7, 0.1);
        assert!(!audio.is_playing(7));
        let events = audio.take_events();
        assert_eq!(
            events,
            vec![
                AudioEvent::Started { emitter: 7, sound },
                AudioEvent::Stopped { emitter: 7 }
            ]
        );
        audio.clear_emitters();
        assert_eq!(audio.emitter_count(), 0);
    }

    #[test]
    fn silent_mode_keeps_logical_state_and_bad_data_is_reported() {
        let mut audio: Audio<u32, MockBackend> = Audio::silent();
        assert!(!audio.has_device());
        let sound = AssetId::new();
        audio.load(sound, tone()).unwrap();
        audio.add_emitter(1, AudioSourceComponent::new(sound), Vec3::ZERO);
        assert!(!audio.is_playing(1));
        audio.play(1, 0.0);
        assert!(audio.is_playing(1));
        // A 0.1 s one-shot ends by logical time when there is no device.
        audio.update(0.2);
        assert!(!audio.is_playing(1));
        assert!(
            audio
                .load(AssetId::new(), Arc::from(&b"RIFF junk"[..]))
                .is_err()
        );
        assert_eq!(linear_to_db(1.0), 0.0);
        assert_eq!(linear_to_db(0.0), -60.0);
    }
}
