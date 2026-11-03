//! Keyboard and mouse state sampled for fixed simulation ticks.
//!
//! Window events arrive at frame rate, simulation runs at a fixed rate, and a
//! frame may run zero, one or several ticks. To keep button edges reliable:
//!
//! - A press is recorded as an edge and delivered to exactly one tick: the
//!   first tick that runs after it. If a frame runs no ticks, the edge waits.
//! - A key pressed and released between two ticks still produces one press
//!   edge, even though it is no longer held.
//! - Key-repeat events from the OS are ignored.
//! - Mouse motion accumulates and is consumed by the first tick, so look speed
//!   does not depend on the number of ticks per frame.

use std::collections::HashSet;

use glam::Vec2;
use winit::keyboard::KeyCode;

/// Input as seen by one simulation tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TickInput {
    held: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    /// Raw mouse motion (device units) since the previous tick.
    pub mouse_delta: Vec2,
}

impl TickInput {
    pub fn held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    /// True only on the tick that first observes the press.
    pub fn pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }
}

/// Accumulates window events between ticks.
#[derive(Debug, Default)]
pub struct InputState {
    held: HashSet<KeyCode>,
    pending_pressed: HashSet<KeyCode>,
    pending_mouse: Vec2,
}

impl InputState {
    pub fn key_down(&mut self, key: KeyCode, repeat: bool) {
        if repeat {
            return;
        }
        if self.held.insert(key) {
            self.pending_pressed.insert(key);
        }
    }

    pub fn key_up(&mut self, key: KeyCode) {
        self.held.remove(&key);
    }

    pub fn mouse_moved(&mut self, delta: Vec2) {
        if delta.is_finite() {
            self.pending_mouse += delta;
        }
    }

    /// Releases everything, e.g. on focus loss, so keys do not stick.
    pub fn clear(&mut self) {
        self.held.clear();
        self.pending_pressed.clear();
        self.pending_mouse = Vec2::ZERO;
    }

    /// Produces the input for the next tick and consumes pending edges/motion.
    pub fn begin_tick(&mut self) -> TickInput {
        TickInput {
            held: self.held.clone(),
            pressed: std::mem::take(&mut self.pending_pressed),
            mouse_delta: std::mem::take(&mut self.pending_mouse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_is_delivered_to_exactly_one_tick() {
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyE, false);
        let t1 = input.begin_tick();
        let t2 = input.begin_tick();
        assert!(t1.pressed(KeyCode::KeyE) && t1.held(KeyCode::KeyE));
        assert!(!t2.pressed(KeyCode::KeyE) && t2.held(KeyCode::KeyE));
    }

    #[test]
    fn press_waits_for_a_frame_with_no_ticks() {
        let mut input = InputState::default();
        input.key_down(KeyCode::Space, false);
        // Frame with zero ticks: nobody calls begin_tick.
        input.key_down(KeyCode::KeyW, false);
        let t = input.begin_tick();
        assert!(t.pressed(KeyCode::Space));
        assert!(t.pressed(KeyCode::KeyW));
    }

    #[test]
    fn tap_between_ticks_is_not_lost() {
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyE, false);
        input.key_up(KeyCode::KeyE);
        let t = input.begin_tick();
        assert!(t.pressed(KeyCode::KeyE));
        assert!(!t.held(KeyCode::KeyE));
    }

    #[test]
    fn os_key_repeat_does_not_create_new_edges() {
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyE, false);
        input.begin_tick();
        input.key_down(KeyCode::KeyE, true);
        input.key_down(KeyCode::KeyE, false); // duplicate down without up
        assert!(!input.begin_tick().pressed(KeyCode::KeyE));
    }

    #[test]
    fn mouse_motion_accumulates_and_is_consumed_once() {
        let mut input = InputState::default();
        input.mouse_moved(Vec2::new(3.0, 1.0));
        input.mouse_moved(Vec2::new(2.0, -4.0));
        input.mouse_moved(Vec2::new(f32::NAN, 0.0));
        assert_eq!(input.begin_tick().mouse_delta, Vec2::new(5.0, -3.0));
        assert_eq!(input.begin_tick().mouse_delta, Vec2::ZERO);
    }

    #[test]
    fn clear_releases_held_keys() {
        let mut input = InputState::default();
        input.key_down(KeyCode::KeyW, false);
        input.clear();
        let t = input.begin_tick();
        assert!(!t.held(KeyCode::KeyW) && !t.pressed(KeyCode::KeyW));
    }
}
