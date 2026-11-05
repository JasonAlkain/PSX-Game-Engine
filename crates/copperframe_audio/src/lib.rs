//! The audio mixer shared by the game and the editor, and the editor's
//! sound preview player.
//!
//! [`Mixer`] owns a kira [`AudioManager`] with one sub-track per
//! [`AudioBus`] under the main (master) track. Each bus gets its volume
//! from the project's [`AudioSettings`] (`project.json` `audio`), times the
//! player's volume sliders; muted buses are silent, and while any bus is
//! soloed only soloed buses play (solo is an editor monitoring aid and is
//! not saved). Every bus and the master carry a [`Meter`] after their fader
//! (peak level and a sticky clip flag, read without locking), and the
//! master has an optional limiter (a fast, high-ratio compressor and a
//! ceiling that clamps what it lets through) that holds the output under
//! 0 dBFS.
//!
//! [`Preview`] plays one sound at a time through its own mixer, for the
//! editor's Assets panel. It never shares a device or tracks with a running
//! game (the game is a separate process), and drops the decoded sound as
//! soon as playback stops.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

pub use copperframe_assets::project::{AudioSettings, BusSettings};
use copperframe_core::AssetId;
pub use copperframe_scene::AudioBus;
use kira::backend::Backend;
use kira::effect::compressor::{CompressorBuilder, CompressorHandle};
use kira::effect::volume_control::{VolumeControlBuilder, VolumeControlHandle};
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::sound::PlaybackState;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{MainTrackBuilder, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, Frame, Mix, Tween};

/// Linear gain (0..=1) to decibels; at or below 0.001 is silence.
pub fn linear_to_db(v: f32) -> f32 {
    if v <= 0.001 {
        Decibels::SILENCE.0
    } else {
        (20.0 * v.log10()).max(Decibels::SILENCE.0)
    }
}

/// Peak amplitude (1.0 = 0 dBFS) to decibels, for meters.
pub fn peak_to_db(peak: f32) -> f32 {
    if peak <= 0.0 {
        Decibels::SILENCE.0
    } else {
        (20.0 * peak.log10()).max(Decibels::SILENCE.0)
    }
}

fn tween(secs: f32) -> Tween {
    Tween {
        duration: Duration::from_secs_f32(secs.max(0.0)),
        ..Default::default()
    }
}

/// The limiter's threshold, in dBFS.
pub const LIMITER_THRESHOLD_DB: f64 = -1.0;

#[derive(Debug, Default)]
struct MeterState {
    /// Highest absolute sample since the last read, as f32 bits.
    peak: AtomicU32,
    clipped: AtomicBool,
}

/// A level meter fed from the audio thread. Cheap to clone; every clone
/// reads the same meter.
#[derive(Debug, Clone, Default)]
pub struct Meter(Arc<MeterState>);

impl Meter {
    /// The highest sample level (1.0 = 0 dBFS) since the last call.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.0.peak.swap(0, Ordering::Relaxed))
    }

    /// Whether a sample reached 0 dBFS or more since the last reset.
    pub fn clipped(&self) -> bool {
        self.0.clipped.load(Ordering::Relaxed)
    }

    pub fn reset_clip(&self) {
        self.0.clipped.store(false, Ordering::Relaxed);
    }

    fn builder(&self) -> MeterBuilder {
        MeterBuilder(self.0.clone())
    }
}

struct MeterBuilder(Arc<MeterState>);

impl EffectBuilder for MeterBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        (Box::new(MeterEffect(self.0)), ())
    }
}

struct MeterEffect(Arc<MeterState>);

impl Effect for MeterEffect {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        let mut peak = 0.0f32;
        for f in input.iter() {
            peak = peak.max(f.left.abs()).max(f.right.abs());
        }
        if !peak.is_finite() {
            return;
        }
        // Non-negative floats order like their bit patterns.
        self.0.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if peak >= 1.0 {
            self.0.clipped.store(true, Ordering::Relaxed);
        }
    }
}

/// The limiter's last stage: clamps samples to the threshold so peaks the
/// compressor's attack lets through still never reach 0 dBFS.
struct CeilingBuilder(Arc<AtomicBool>);

impl EffectBuilder for CeilingBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        (Box::new(Ceiling(self.0)), ())
    }
}

struct Ceiling(Arc<AtomicBool>);

impl Effect for Ceiling {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        if !self.0.load(Ordering::Relaxed) {
            return;
        }
        let max = 10f32.powf(LIMITER_THRESHOLD_DB as f32 / 20.0);
        for f in input.iter_mut() {
            f.left = f.left.clamp(-max, max);
            f.right = f.right.clamp(-max, max);
        }
    }
}

/// A mixer strip: the master or one bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strip {
    Master,
    Bus(AudioBus),
}

impl Strip {
    pub const ALL: [Strip; 5] = [
        Strip::Master,
        Strip::Bus(AudioBus::Music),
        Strip::Bus(AudioBus::Effects),
        Strip::Bus(AudioBus::Voice),
        Strip::Bus(AudioBus::Ambient),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Strip::Master => "Master",
            Strip::Bus(b) => b.label(),
        }
    }

    fn index(self) -> usize {
        match self {
            Strip::Master => 0,
            Strip::Bus(b) => 1 + b.index(),
        }
    }
}

/// Player volume sliders (linear 0..=1) multiplied onto the mixer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerGains {
    pub master: f32,
    pub music: f32,
    /// Sound effects, voice and ambience.
    pub effects: f32,
}

impl Default for PlayerGains {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            effects: 1.0,
        }
    }
}

/// The buses, their meters and the master limiter on one audio device.
pub struct Mixer<B: Backend> {
    manager: AudioManager<B>,
    tracks: [TrackHandle; 4],
    /// Master then bus faders. They sit before the meters, so meters read
    /// after the fader (what the strip sends on).
    faders: [VolumeControlHandle; 5],
    meters: [Meter; 5],
    limiter: CompressorHandle,
    ceiling: Arc<AtomicBool>,
    settings: AudioSettings,
    player: PlayerGains,
    solo: [bool; 4],
}

impl<B: Backend> Mixer<B>
where
    B::Error: std::fmt::Debug,
{
    /// Opens the device with the buses set up from `audio`. `settings`'
    /// main track builder is replaced.
    pub fn open(
        mut settings: AudioManagerSettings<B>,
        audio: AudioSettings,
    ) -> Result<Self, String> {
        let meters: [Meter; 5] = Default::default();
        let ceiling = Arc::new(AtomicBool::new(audio.limiter));
        let mut main = MainTrackBuilder::new();
        let master = main.add_effect(VolumeControlBuilder::default());
        let limiter = main.add_effect(
            CompressorBuilder::new()
                .threshold(LIMITER_THRESHOLD_DB)
                .ratio(20.0)
                .attack_duration(Duration::from_millis(1))
                .release_duration(Duration::from_millis(80)),
        );
        main.add_effect(CeilingBuilder(Arc::clone(&ceiling)));
        main.add_effect(meters[0].builder());
        settings.main_track_builder = main;
        let mut manager =
            AudioManager::<B>::new(settings).map_err(|e| format!("no audio output ({e:?})"))?;
        let mut track = |i: usize| {
            let mut b = TrackBuilder::new();
            let fader = b.add_effect(VolumeControlBuilder::default());
            b.add_effect(meters[i].builder());
            manager
                .add_sub_track(b)
                .map(|t| (t, fader))
                .map_err(|e| e.to_string())
        };
        let (t1, f1) = track(1)?;
        let (t2, f2) = track(2)?;
        let (t3, f3) = track(3)?;
        let (t4, f4) = track(4)?;
        let mut mixer = Self {
            manager,
            tracks: [t1, t2, t3, t4],
            faders: [master, f1, f2, f3, f4],
            meters,
            limiter,
            ceiling,
            settings: audio,
            player: PlayerGains::default(),
            solo: [false; 4],
        };
        mixer.apply(0.0);
        Ok(mixer)
    }

    pub fn manager(&mut self) -> &mut AudioManager<B> {
        &mut self.manager
    }

    /// The track sounds on `bus` play on (spatial emitters add sub-tracks
    /// to it).
    pub fn track(&mut self, bus: AudioBus) -> &mut TrackHandle {
        &mut self.tracks[bus.index()]
    }

    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: AudioSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.apply(0.05);
        }
    }

    pub fn set_player_gains(&mut self, gains: PlayerGains) {
        if gains != self.player {
            self.player = gains;
            self.apply(0.05);
        }
    }

    pub fn solo(&self, bus: AudioBus) -> bool {
        self.solo[bus.index()]
    }

    pub fn set_solo(&mut self, bus: AudioBus, on: bool) {
        if self.solo[bus.index()] != on {
            self.solo[bus.index()] = on;
            self.apply(0.02);
        }
    }

    pub fn meter(&self, strip: Strip) -> &Meter {
        &self.meters[strip.index()]
    }

    /// Pauses or resumes gameplay sound (effects, voice, ambience); music
    /// keeps playing.
    pub fn set_gameplay_paused(&mut self, paused: bool) {
        for bus in [AudioBus::Effects, AudioBus::Voice, AudioBus::Ambient] {
            let t = &mut self.tracks[bus.index()];
            if paused {
                t.pause(tween(0.1));
            } else {
                t.resume(tween(0.1));
            }
        }
    }

    /// The gain in dB each strip currently gets (master first, then
    /// [`AudioBus::ALL`] order); silence is [`Decibels::SILENCE`].
    pub fn gains_db(&self) -> [f32; 5] {
        let s = &self.settings;
        let silence = Decibels::SILENCE.0;
        let any_solo = self.solo.iter().any(|&x| x);
        let mut out = [silence; 5];
        out[0] = if s.master.mute {
            silence
        } else {
            (s.master.volume_db + linear_to_db(self.player.master)).max(silence)
        };
        for bus in AudioBus::ALL {
            let b = s.bus(bus);
            let player = match bus {
                AudioBus::Music => self.player.music,
                _ => self.player.effects,
            };
            let audible = !b.mute && (!any_solo || self.solo[bus.index()]);
            out[1 + bus.index()] = if audible {
                (b.volume_db + linear_to_db(player)).max(silence)
            } else {
                silence
            };
        }
        out
    }

    fn apply(&mut self, secs: f32) {
        let gains = self.gains_db();
        for (fader, db) in self.faders.iter_mut().zip(gains) {
            fader.set_volume(Decibels(db), tween(secs));
        }
        self.ceiling.store(self.settings.limiter, Ordering::Relaxed);
        let mix = if self.settings.limiter {
            Mix::WET
        } else {
            Mix::DRY
        };
        self.limiter.set_mix(mix, Tween::default());
    }
}

/// Decodes a WAV or Ogg Vorbis file.
pub fn decode(bytes: Arc<[u8]>) -> Result<StaticSoundData, String> {
    StaticSoundData::from_cursor(Cursor::new(bytes))
        .map_err(|e| format!("could not decode sound: {e}"))
}

/// The sound a [`Preview`] is playing.
struct Playing {
    asset: AssetId,
    handle: StaticSoundHandle,
    duration: f64,
}

/// What the preview is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewState {
    pub asset: AssetId,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
}

/// The editor's sound preview: one sound at a time, through its own mixer
/// (so bus volumes, mute and solo are heard), released when it stops.
pub struct Preview<B: Backend> {
    mixer: Mixer<B>,
    playing: Option<Playing>,
    volume_db: f32,
}

impl<B: Backend> Preview<B>
where
    B::Error: std::fmt::Debug,
{
    pub fn open(settings: AudioManagerSettings<B>, audio: AudioSettings) -> Result<Self, String> {
        Ok(Self {
            mixer: Mixer::open(settings, audio)?,
            playing: None,
            volume_db: 0.0,
        })
    }

    pub fn mixer(&mut self) -> &mut Mixer<B> {
        &mut self.mixer
    }

    pub fn mixer_ref(&self) -> &Mixer<B> {
        &self.mixer
    }

    /// Plays `bytes` (the asset's file) on `bus` from `start` seconds,
    /// stopping whatever was playing.
    pub fn play(
        &mut self,
        asset: AssetId,
        bytes: Arc<[u8]>,
        bus: AudioBus,
        start: f64,
    ) -> Result<(), String> {
        self.stop();
        let data = decode(bytes)?;
        let duration = data.duration().as_secs_f64();
        let data = data
            .volume(Decibels(self.volume_db))
            .start_position(start.clamp(0.0, duration));
        let handle = self
            .mixer
            .track(bus)
            .play(data)
            .map_err(|e| format!("could not play the sound: {e}"))?;
        self.playing = Some(Playing {
            asset,
            handle,
            duration,
        });
        Ok(())
    }

    pub fn pause(&mut self) {
        if let Some(p) = &mut self.playing {
            p.handle.pause(tween(0.02));
        }
    }

    pub fn resume(&mut self) {
        if let Some(p) = &mut self.playing {
            p.handle.resume(tween(0.02));
        }
    }

    /// Stops and releases the sound.
    pub fn stop(&mut self) {
        if let Some(mut p) = self.playing.take() {
            p.handle.stop(tween(0.02));
        }
    }

    pub fn seek(&mut self, secs: f64) {
        if let Some(p) = &mut self.playing {
            p.handle.seek_to(secs.clamp(0.0, p.duration));
        }
    }

    /// Preview gain in dB (on top of the mixer).
    pub fn set_volume_db(&mut self, db: f32) {
        self.volume_db = db;
        if let Some(p) = &mut self.playing {
            p.handle.set_volume(Decibels(db), tween(0.03));
        }
    }

    pub fn volume_db(&self) -> f32 {
        self.volume_db
    }

    /// Forgets a sound that has finished. Call once a frame.
    pub fn update(&mut self) {
        if self
            .playing
            .as_ref()
            .is_some_and(|p| p.handle.state() == PlaybackState::Stopped)
        {
            self.playing = None;
        }
    }

    pub fn state(&self) -> Option<PreviewState> {
        let p = self.playing.as_ref()?;
        let state = p.handle.state();
        Some(PreviewState {
            asset: p.asset,
            position: p.handle.position(),
            duration: p.duration,
            paused: matches!(state, PlaybackState::Paused | PlaybackState::Pausing),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::backend::mock::{MockBackend, MockBackendSettings};

    pub(crate) fn tone(amplitude: f32, secs: f32) -> Arc<[u8]> {
        let n = (44100.0 * secs) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| amplitude * (i as f32 * 0.05).sin())
            .collect();
        copperframe_assets::import::audio::wav_bytes(44100, &samples).into()
    }

    fn mock_settings() -> AudioManagerSettings<MockBackend> {
        AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: 44100 },
            ..Default::default()
        }
    }

    fn run(mixer: &mut Mixer<MockBackend>, blocks: usize) {
        // Like a device callback: start processing, then fill a buffer.
        let b = mixer.manager().backend_mut();
        for _ in 0..blocks {
            b.on_start_processing();
            b.process();
        }
    }

    fn peaks(mixer: &Mixer<MockBackend>) -> [f32; 5] {
        Strip::ALL.map(|s| mixer.meter(s).take_peak())
    }

    #[test]
    fn buses_route_to_master_and_mute_and_solo_silence_them() {
        let mut m = Mixer::open(mock_settings(), AudioSettings::default()).unwrap();
        let sound = decode(tone(0.5, 2.0)).unwrap();
        m.track(AudioBus::Music).play(sound.clone()).unwrap();
        run(&mut m, 400);
        let p = peaks(&m);
        assert!(p[0] > 0.1, "master hears the music bus: {p:?}");
        assert!(p[1] > 0.1, "music meter: {p:?}");
        assert_eq!(p[2], 0.0, "nothing on SFX");

        // Muting the bus silences it and the master.
        let mut s = AudioSettings::default();
        s.music.mute = true;
        m.set_settings(s);
        run(&mut m, 400);
        peaks(&m);
        run(&mut m, 200);
        let p = peaks(&m);
        assert!(p[0] < 0.01, "muted: {p:?}");

        // Soloing another bus silences the music bus too.
        m.set_settings(AudioSettings::default());
        m.set_solo(AudioBus::Voice, true);
        run(&mut m, 400);
        peaks(&m);
        run(&mut m, 200);
        assert!(peaks(&m)[0] < 0.01);
        assert_eq!(m.gains_db()[1], Decibels::SILENCE.0);
        m.set_solo(AudioBus::Voice, false);
        assert_eq!(m.gains_db()[1], 0.0);
    }

    #[test]
    fn volumes_combine_settings_and_player_sliders() {
        let mut s = AudioSettings::default();
        s.master.volume_db = -6.0;
        s.effects.volume_db = 3.0;
        let mut m = Mixer::open(mock_settings(), s).unwrap();
        m.set_player_gains(PlayerGains {
            master: 1.0,
            music: 0.0,
            effects: 0.5,
        });
        let g = m.gains_db();
        assert_eq!(g[0], -6.0);
        assert_eq!(g[1], Decibels::SILENCE.0, "music slider at zero");
        assert!((g[2] - (3.0 + linear_to_db(0.5))).abs() < 1e-4);
        assert!(
            (g[3] - linear_to_db(0.5)).abs() < 1e-4,
            "voice follows effects"
        );
    }

    #[test]
    fn hot_signals_clip_without_the_limiter_and_are_held_with_it() {
        // Two loud sounds on one bus sum past 0 dBFS. Returns the master
        // peak after the limiter's attack.
        let loud = |limiter: bool| {
            let s = AudioSettings {
                limiter,
                ..Default::default()
            };
            let mut m = Mixer::open(mock_settings(), s).unwrap();
            let sound = decode(tone(0.9, 1.0)).unwrap();
            m.track(AudioBus::Effects).play(sound.clone()).unwrap();
            m.track(AudioBus::Effects).play(sound).unwrap();
            run(&mut m, 100);
            m.meter(Strip::Master).take_peak();
            run(&mut m, 100);
            (m.meter(Strip::Master).take_peak(), m)
        };
        let (open, m) = loud(false);
        assert!(open > 1.5, "unlimited sum {open}");
        assert!(m.meter(Strip::Master).clipped());
        m.meter(Strip::Master).reset_clip();
        assert!(!m.meter(Strip::Master).clipped());
        let (limited, m) = loud(true);
        assert!(limited < open * 0.75, "limited {limited} vs {open}");
        let ceiling = 10f32.powf(LIMITER_THRESHOLD_DB as f32 / 20.0);
        assert!(limited <= ceiling + 1e-4, "limited {limited}");
        assert!(!m.meter(Strip::Master).clipped());
        assert!(
            m.meter(Strip::Bus(AudioBus::Effects)).clipped(),
            "the bus itself is hot"
        );
    }

    #[test]
    fn meters_read_after_the_fader() {
        let peak = |db: f32| {
            let mut s = AudioSettings::default();
            s.effects.volume_db = db;
            s.limiter = false;
            let mut m = Mixer::open(mock_settings(), s).unwrap();
            m.track(AudioBus::Effects)
                .play(decode(tone(0.25, 1.0)).unwrap())
                .unwrap();
            run(&mut m, 50);
            m.meter(Strip::Bus(AudioBus::Effects)).take_peak();
            run(&mut m, 50);
            m.meter(Strip::Bus(AudioBus::Effects)).take_peak()
        };
        let (flat, up) = (peak(0.0), peak(12.0));
        assert!(flat > 0.2 && flat < 0.3, "{flat}");
        assert!(
            (up / flat - 10f32.powf(12.0 / 20.0)).abs() < 0.1,
            "{up} vs {flat}"
        );
    }

    #[test]
    fn preview_plays_pauses_seeks_and_releases() {
        let mut p = Preview::open(mock_settings(), AudioSettings::default()).unwrap();
        let asset = AssetId::new();
        p.play(asset, tone(0.5, 1.0), AudioBus::Effects, 0.0)
            .unwrap();
        run(p.mixer(), 50);
        let s = p.state().unwrap();
        assert_eq!(s.asset, asset);
        assert!((s.duration - 1.0).abs() < 0.01);
        assert!(s.position > 0.0 && !s.paused);
        p.seek(0.5);
        run(p.mixer(), 2);
        assert!(p.state().unwrap().position >= 0.5);
        p.pause();
        run(p.mixer(), 50);
        assert!(p.state().unwrap().paused);
        p.resume();
        p.set_volume_db(-12.0);
        assert_eq!(p.volume_db(), -12.0);
        p.stop();
        assert!(p.state().is_none());
        // A sound that plays to the end is released by update().
        p.play(asset, tone(0.5, 0.05), AudioBus::Music, 0.0)
            .unwrap();
        run(p.mixer(), 200);
        p.update();
        assert!(p.state().is_none());
        assert!(
            p.play(asset, Arc::from(&b"junk"[..]), AudioBus::Music, 0.0)
                .is_err()
        );
    }

    #[test]
    fn meters_convert_to_decibels() {
        assert_eq!(peak_to_db(1.0), 0.0);
        assert_eq!(peak_to_db(0.0), -60.0);
        assert!((peak_to_db(0.5) + 6.02).abs() < 0.01);
    }
}
