//! Fixed-step simulation timing.
//!
//! Rendering runs at whatever rate the display allows. Simulation advances in
//! fixed increments (60 Hz by default) using an accumulator, so a slower
//! presentation rate never changes movement speed. Catch-up work per frame is
//! bounded: after a long stall (debugger break, window drag) the excess time is
//! dropped instead of running hundreds of ticks in one frame.

use std::time::Duration;

/// Default simulation rate.
pub const DEFAULT_TICK_HZ: u32 = 60;
/// Highest supported rate: one tick per nanosecond, the resolution of `Duration`.
pub const MAX_TICK_HZ: u32 = 1_000_000_000;
/// Default upper bound on simulation ticks executed for one rendered frame.
pub const DEFAULT_MAX_TICKS_PER_FRAME: u32 = 8;

/// How much simulation work a frame should perform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepPlan {
    /// Number of fixed ticks to run this frame.
    pub ticks: u32,
    /// Interpolation factor in `[0, 1)` between the previous and current
    /// simulation states, for rendering.
    pub alpha: f32,
    /// Simulation time discarded because the catch-up limit was reached.
    pub dropped: Duration,
}

/// Accumulator for a fixed simulation step.
#[derive(Debug, Clone)]
pub struct FixedStep {
    step: Duration,
    max_ticks_per_frame: u32,
    accumulator: Duration,
    total_ticks: u64,
}

impl Default for FixedStep {
    fn default() -> Self {
        Self::from_hz(DEFAULT_TICK_HZ)
    }
}

impl FixedStep {
    /// Creates a stepper running at `hz` ticks per second.
    ///
    /// `hz` is clamped to `1..=MAX_TICK_HZ` so the step is always a finite,
    /// non-zero duration.
    pub fn from_hz(hz: u32) -> Self {
        let hz = hz.clamp(1, MAX_TICK_HZ);
        Self {
            step: Duration::from_secs(1) / hz,
            max_ticks_per_frame: DEFAULT_MAX_TICKS_PER_FRAME,
            accumulator: Duration::ZERO,
            total_ticks: 0,
        }
    }

    /// Sets the per-frame catch-up limit (minimum one tick).
    pub fn with_max_ticks_per_frame(mut self, max: u32) -> Self {
        self.max_ticks_per_frame = max.max(1);
        self
    }

    /// Length of one simulation tick.
    pub fn step(&self) -> Duration {
        self.step
    }

    /// Length of one simulation tick in seconds.
    pub fn step_secs(&self) -> f32 {
        self.step.as_secs_f32()
    }

    /// Total ticks planned since creation.
    pub fn total_ticks(&self) -> u64 {
        self.total_ticks
    }

    /// Adds `frame_time` to the accumulator and returns how many ticks to run.
    pub fn advance(&mut self, frame_time: Duration) -> StepPlan {
        self.accumulator += frame_time;

        let mut ticks = 0;
        while self.accumulator >= self.step && ticks < self.max_ticks_per_frame {
            self.accumulator -= self.step;
            ticks += 1;
        }

        // Keep at most one partial step after hitting the limit; anything more
        // would only grow without bound while the machine cannot keep up.
        let mut dropped = Duration::ZERO;
        if self.accumulator >= self.step {
            let kept =
                Duration::from_nanos((self.accumulator.as_nanos() % self.step.as_nanos()) as u64);
            dropped = self.accumulator - kept;
            self.accumulator = kept;
        }

        self.total_ticks += u64::from(ticks);
        StepPlan {
            ticks,
            alpha: (self.accumulator.as_secs_f64() / self.step.as_secs_f64()) as f32,
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn sixty_hz_frames_run_one_tick_each() {
        let mut s = FixedStep::from_hz(60);
        let frame = s.step();
        for _ in 0..10 {
            assert_eq!(s.advance(frame).ticks, 1);
        }
        assert_eq!(s.total_ticks(), 10);
    }

    #[test]
    fn thirty_fps_presentation_keeps_simulation_rate() {
        let mut fast = FixedStep::from_hz(60);
        let mut slow = FixedStep::from_hz(60);
        let mut fast_ticks = 0;
        let mut slow_ticks = 0;
        for _ in 0..120 {
            fast_ticks += fast.advance(Duration::from_secs(1) / 60).ticks;
        }
        for _ in 0..60 {
            slow_ticks += slow.advance(Duration::from_secs(1) / 30).ticks;
        }
        assert_eq!(fast_ticks, 120);
        assert_eq!(slow_ticks, 120);
    }

    #[test]
    fn partial_steps_accumulate_and_report_alpha() {
        let mut s = FixedStep::from_hz(10); // 100 ms step
        let p = s.advance(ms(40));
        assert_eq!(p.ticks, 0);
        assert!((p.alpha - 0.4).abs() < 1e-4);
        let p = s.advance(ms(70));
        assert_eq!(p.ticks, 1);
        assert!((p.alpha - 0.1).abs() < 1e-4);
    }

    #[test]
    fn long_stall_is_bounded_and_dropped_time_reported() {
        let mut s = FixedStep::from_hz(10).with_max_ticks_per_frame(4);
        let p = s.advance(ms(2_050));
        assert_eq!(p.ticks, 4);
        assert_eq!(p.dropped, ms(1_600));
        assert!((p.alpha - 0.5).abs() < 1e-4);
        // Next normal frame behaves normally.
        assert_eq!(s.advance(ms(100)).ticks, 1);
    }

    #[test]
    fn zero_hz_does_not_divide_by_zero() {
        let s = FixedStep::from_hz(0);
        assert_eq!(s.step(), Duration::from_secs(1));
    }

    #[test]
    fn rates_above_one_ghz_keep_a_nonzero_step() {
        let mut s = FixedStep::from_hz(u32::MAX);
        assert_eq!(s.step(), Duration::from_nanos(1));
        assert_eq!(s.advance(Duration::ZERO).ticks, 0);
        assert_eq!(s.advance(Duration::from_nanos(3)).ticks, 3);
    }
}
