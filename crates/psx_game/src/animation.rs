//! Runtime skeletal animation: one [`Animator`] per entity with an
//! `animator` component and a skinned model.
//!
//! Each tick the animator advances its clip, crossfades from the previous
//! clip when a fade is running, and computes the joint palette the renderer
//! uploads. Clip changes come from gameplay code through
//! [`crate::Ctx::play_animation`].

use std::sync::Arc;

use glam::Mat4;
use psx_assets::ImportedModel;
use psx_assets::anim::Pose;

/// One clip being played.
#[derive(Debug, Clone, PartialEq)]
struct Track {
    clip: usize,
    /// Seconds of playback (scaled by speed), before wrapping/clamping.
    elapsed: f32,
    speed: f32,
    looping: bool,
    /// `false` holds the current frame.
    running: bool,
}

impl Track {
    fn local_time(&self, model: &ImportedModel) -> f32 {
        model
            .clips
            .get(self.clip)
            .map_or(0.0, |c| c.local_time(self.elapsed, self.looping))
    }

    /// The clip has played to its end (non-looping clips only; reversed
    /// playback ends at the start).
    fn ended(&self, model: &ImportedModel) -> bool {
        let Some(c) = model.clips.get(self.clip) else {
            return true;
        };
        !self.looping
            && ((self.speed > 0.0 && self.elapsed >= c.duration)
                || (self.speed < 0.0 && self.elapsed <= 0.0))
    }
}

/// Animation state of one entity (an hecs component).
pub struct Animator {
    model: Arc<ImportedModel>,
    current: Option<Track>,
    /// Track being faded out (`None` = the rest pose), with fade progress
    /// (seconds) and length.
    fading: Option<(Option<Track>, f32, f32)>,
    finished_reported: bool,
    palette: Vec<Mat4>,
}

/// What [`Animator::advance`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub clip: String,
}

impl Animator {
    /// An animator for `model`, or `None` when the model has no skeleton.
    pub fn new(model: Arc<ImportedModel>) -> Option<Self> {
        model.skeleton.as_ref()?;
        let palette = model.rest_palette();
        Some(Self {
            model,
            current: None,
            fading: None,
            finished_reported: false,
            palette,
        })
    }

    pub fn clip_names(&self) -> Vec<&str> {
        self.model.clips.iter().map(|c| c.name.as_str()).collect()
    }

    /// Starts `clip`, crossfading from the current pose over `fade_secs`
    /// (0 = cut). `running == false` holds the first frame. Returns
    /// `false` (and changes nothing) when the model has no such clip.
    pub fn play(
        &mut self,
        clip: &str,
        fade_secs: f32,
        speed: f32,
        looping: bool,
        running: bool,
    ) -> bool {
        let Some(index) = self.model.clips.iter().position(|c| c.name == clip) else {
            return false;
        };
        let speed = if speed.is_finite() {
            speed.clamp(-10.0, 10.0)
        } else {
            1.0
        };
        let duration = self.model.clips[index].duration;
        let track = Track {
            clip: index,
            elapsed: if speed < 0.0 { duration } else { 0.0 },
            speed,
            looping,
            running,
        };
        self.start(Some(track), fade_secs);
        true
    }

    /// Fades back to the rest pose.
    pub fn stop(&mut self, fade_secs: f32) {
        self.start(None, fade_secs);
    }

    fn start(&mut self, track: Option<Track>, fade_secs: f32) {
        let fade = if fade_secs.is_finite() {
            fade_secs.clamp(0.0, 10.0)
        } else {
            0.0
        };
        let old = self.current.take();
        self.fading = (fade > 0.0).then_some((old, 0.0, fade));
        self.current = track;
        self.finished_reported = false;
        self.update_palette();
    }

    /// The playing clip's name and clip-local time.
    pub fn state(&self) -> Option<(&str, f32)> {
        let t = self.current.as_ref()?;
        let clip = self.model.clips.get(t.clip)?;
        Some((clip.name.as_str(), t.local_time(&self.model)))
    }

    /// `true` while a clip is running and has not reached its end.
    pub fn is_playing(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|t| t.running && !t.ended(&self.model))
    }

    pub fn palette(&self) -> &[Mat4] {
        &self.palette
    }

    /// Advances by `dt` seconds. Returns the clip that reached its end
    /// during this step (non-looping clips, reported once).
    pub fn advance(&mut self, dt: f32) -> Option<Finished> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if let Some((track, progress, length)) = &mut self.fading {
            if let Some(track) = track {
                step(track, dt, &self.model);
            }
            *progress += dt;
            if *progress >= *length {
                self.fading = None;
            }
        }
        let mut finished = None;
        if let Some(track) = &mut self.current {
            step(track, dt, &self.model);
            if track.running && track.ended(&self.model) && !self.finished_reported {
                self.finished_reported = true;
                finished = self.model.clips.get(track.clip).map(|c| Finished {
                    clip: c.name.clone(),
                });
            }
        }
        self.update_palette();
        finished
    }

    fn pose_of(&self, track: Option<&Track>) -> Pose {
        let Some(skeleton) = self.model.skeleton.as_ref() else {
            return Pose { locals: Vec::new() };
        };
        let mut pose = skeleton.rest_pose();
        if let Some(t) = track
            && let Some(clip) = self.model.clips.get(t.clip)
        {
            clip.sample_into(&mut pose, t.local_time(&self.model));
        }
        pose
    }

    fn update_palette(&mut self) {
        let Some(skeleton) = self.model.skeleton.as_ref() else {
            return;
        };
        let target = self.pose_of(self.current.as_ref());
        let pose = match &self.fading {
            Some((old, progress, length)) => {
                let from = self.pose_of(old.as_ref());
                Pose::blend(&from, &target, progress / length)
            }
            None => target,
        };
        self.palette = skeleton.palette(&pose);
    }
}

fn step(track: &mut Track, dt: f32, model: &ImportedModel) {
    if !track.running {
        return;
    }
    let duration = model.clips.get(track.clip).map_or(0.0, |c| c.duration);
    track.elapsed += dt * track.speed;
    if track.looping {
        // Keep the number small so precision does not degrade over long
        // sessions; local_time wraps the same way.
        if duration > 0.0 {
            track.elapsed = track.elapsed.rem_euclid(duration);
        }
    } else {
        track.elapsed = track.elapsed.clamp(0.0, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use psx_assets::fixtures::Fixture;
    use psx_assets::{ImportSettings, import_gltf};

    fn import(name: &str, f: Fixture) -> ImportedModel {
        let dir = std::env::temp_dir().join(format!(
            "psx_game_anim_{name}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("model.glb");
        std::fs::write(&file, f.to_glb()).unwrap();
        let m = import_gltf(&file, &dir, &ImportSettings::default()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        m
    }

    fn arm() -> Arc<ImportedModel> {
        Arc::new(import("arm", Fixture::skinned_arm()))
    }

    /// World-space y of the bar tip after skinning with the palette, via
    /// the upper joint's matrix applied to the bind-space point (0, 2, 0).
    fn upper_tip(a: &Animator) -> glam::Vec3 {
        let skel = a.model.skeleton.as_ref().unwrap();
        let upper = skel.joint_index("upper").unwrap();
        a.palette()[upper].transform_point3(glam::Vec3::new(0.0, 2.0, 0.0))
    }

    #[test]
    fn plays_clips_and_reports_the_end_once() {
        let mut a = Animator::new(arm()).unwrap();
        assert_eq!(a.clip_names(), vec!["bend", "lift"]);
        assert!(!a.play("missing", 0.0, 1.0, true, true));
        assert!(a.state().is_none());
        // Rest: the tip is straight above the upper joint.
        let rest = upper_tip(&a);
        assert!((rest.x).abs() < 1.0e-4, "{rest}");

        assert!(a.play("bend", 0.0, 1.0, false, true));
        let mut finished = Vec::new();
        for _ in 0..90 {
            if let Some(f) = a.advance(1.0 / 60.0) {
                finished.push(f.clip);
            }
        }
        assert_eq!(finished, vec!["bend".to_owned()]);
        assert!(!a.is_playing());
        let (clip, t) = a.state().unwrap();
        assert_eq!(clip, "bend");
        assert!((t - 1.0).abs() < 1.0e-5);
        // Rotated 90° about +Z: the tip (1 above the joint) points to -x.
        let bent = upper_tip(&a);
        assert!((bent.x + 1.0).abs() < 1.0e-3, "{bent}");
    }

    #[test]
    fn crossfades_between_clips_and_back_to_rest() {
        let mut a = Animator::new(arm()).unwrap();
        assert!(a.play("bend", 0.0, 1.0, false, true));
        for _ in 0..60 {
            a.advance(1.0 / 60.0);
        }
        let bent = upper_tip(&a);
        a.stop(0.5);
        // Half way through the fade the pose is between bent and rest.
        for _ in 0..15 {
            a.advance(1.0 / 60.0);
        }
        let mid = upper_tip(&a);
        assert!(mid.x < -0.1 && mid.x > bent.x + 0.1, "{mid} {bent}");
        for _ in 0..30 {
            a.advance(1.0 / 60.0);
        }
        assert!(upper_tip(&a).x.abs() < 1.0e-4);
        assert!(a.state().is_none());
    }

    #[test]
    fn held_and_reversed_playback() {
        let mut a = Animator::new(arm()).unwrap();
        assert!(a.play("bend", 0.0, 1.0, true, false));
        for _ in 0..30 {
            assert!(a.advance(1.0 / 60.0).is_none());
        }
        assert_eq!(a.state().unwrap().1, 0.0);
        assert!(a.play("bend", 0.0, -2.0, false, true));
        assert_eq!(a.state().unwrap().1, 1.0);
        let mut ends = 0;
        for _ in 0..40 {
            ends += usize::from(a.advance(1.0 / 60.0).is_some());
        }
        assert_eq!(ends, 1);
        assert_eq!(a.state().unwrap().1, 0.0);
        // Non-finite input is ignored rather than poisoning the pose.
        assert!(a.play("bend", f32::NAN, f32::INFINITY, true, true));
        a.advance(f32::NAN);
        assert!(a.palette().iter().all(|m| m.is_finite()));
    }

    #[test]
    fn models_without_a_skeleton_have_no_animator() {
        let m = import("box", Fixture::textured_box([0.5; 3], [1.0; 4], None));
        assert!(Animator::new(Arc::new(m)).is_none());
    }
}
