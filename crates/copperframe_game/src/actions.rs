//! Named input actions.
//!
//! Gameplay reads actions ("move_forward", "interact"), never keys. A
//! versioned bindings file maps each action to keys and mouse buttons:
//!
//! ```json
//! { "format": "psx_input", "version": 1,
//!   "actions": { "move_forward": ["KeyW", "ArrowUp"], "interact": ["KeyE", "MouseLeft"] } }
//! ```
//!
//! The project's `input.json` holds the defaults; the player's remaps are
//! stored in their settings and applied on top. Mouse look is not an action:
//! it is the raw mouse delta scaled by the player's sensitivity.
//!
//! Edges follow the same rules as `copperframe_runtime::input`: a press is
//! delivered to exactly one simulation tick, OS key repeat is ignored, and
//! focus loss releases everything.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use glam::Vec2;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

pub const INPUT_FORMAT: &str = "psx_input";
pub const INPUT_VERSION: u32 = 1;
/// File name of the project's default bindings.
pub const INPUT_FILE: &str = "input.json";

/// Actions every game gets by default (a game may add more in its
/// `input.json`).
pub mod names {
    pub const MOVE_FORWARD: &str = "move_forward";
    pub const MOVE_BACK: &str = "move_back";
    pub const MOVE_LEFT: &str = "move_left";
    pub const MOVE_RIGHT: &str = "move_right";
    pub const RUN: &str = "run";
    pub const JUMP: &str = "jump";
    pub const INTERACT: &str = "interact";
    pub const PAUSE: &str = "pause";
}

/// A physical input that can be bound to an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    Key(KeyCode),
    Mouse(MouseKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseKey {
    Left,
    Right,
    Middle,
}

impl MouseKey {
    pub fn from_winit(b: MouseButton) -> Option<Self> {
        match b {
            MouseButton::Left => Some(MouseKey::Left),
            MouseButton::Right => Some(MouseKey::Right),
            MouseButton::Middle => Some(MouseKey::Middle),
            _ => None,
        }
    }
}

/// Keys that can be named in a bindings file (the winit `KeyCode` names).
const KEYS: &[KeyCode] = {
    use KeyCode::*;
    &[
        KeyA,
        KeyB,
        KeyC,
        KeyD,
        KeyE,
        KeyF,
        KeyG,
        KeyH,
        KeyI,
        KeyJ,
        KeyK,
        KeyL,
        KeyM,
        KeyN,
        KeyO,
        KeyP,
        KeyQ,
        KeyR,
        KeyS,
        KeyT,
        KeyU,
        KeyV,
        KeyW,
        KeyX,
        KeyY,
        KeyZ,
        Digit0,
        Digit1,
        Digit2,
        Digit3,
        Digit4,
        Digit5,
        Digit6,
        Digit7,
        Digit8,
        Digit9,
        ArrowUp,
        ArrowDown,
        ArrowLeft,
        ArrowRight,
        Space,
        Enter,
        Escape,
        Tab,
        Backspace,
        ShiftLeft,
        ShiftRight,
        ControlLeft,
        ControlRight,
        AltLeft,
        AltRight,
        CapsLock,
        F1,
        F2,
        F3,
        F4,
        F5,
        F6,
        F7,
        F8,
        F9,
        F10,
        F11,
        F12,
        Minus,
        Equal,
        BracketLeft,
        BracketRight,
        Semicolon,
        Quote,
        Comma,
        Period,
        Slash,
        Backslash,
        Backquote,
        Insert,
        Delete,
        Home,
        End,
        PageUp,
        PageDown,
        Numpad0,
        Numpad1,
        Numpad2,
        Numpad3,
        Numpad4,
        Numpad5,
        Numpad6,
        Numpad7,
        Numpad8,
        Numpad9,
        NumpadEnter,
    ]
};

impl Button {
    pub fn name(self) -> String {
        match self {
            Button::Key(k) => format!("{k:?}"),
            Button::Mouse(MouseKey::Left) => "MouseLeft".into(),
            Button::Mouse(MouseKey::Right) => "MouseRight".into(),
            Button::Mouse(MouseKey::Middle) => "MouseMiddle".into(),
        }
    }

    pub fn parse(name: &str) -> Option<Button> {
        match name {
            "MouseLeft" => return Some(Button::Mouse(MouseKey::Left)),
            "MouseRight" => return Some(Button::Mouse(MouseKey::Right)),
            "MouseMiddle" => return Some(Button::Mouse(MouseKey::Middle)),
            _ => {}
        }
        KEYS.iter()
            .find(|k| format!("{k:?}") == name)
            .map(|k| Button::Key(*k))
    }

    /// Whether this key can be bound (it appears in the key list).
    pub fn bindable(key: KeyCode) -> bool {
        KEYS.contains(&key)
    }

    /// Short label for menus ("W", "Space", "Mouse Left").
    pub fn label(self) -> String {
        let n = self.name();
        if let Some(rest) = n.strip_prefix("Key") {
            return rest.to_owned();
        }
        if let Some(rest) = n.strip_prefix("Digit") {
            return rest.to_owned();
        }
        if let Some(rest) = n.strip_prefix("Mouse") {
            return format!("Mouse {rest}");
        }
        n
    }
}

#[derive(Debug, Error)]
pub enum BindingsError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not a valid input bindings file: {message}")]
    Invalid { path: String, message: String },
}

/// Action name → buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    pub actions: BTreeMap<String, Vec<Button>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingsFile {
    format: String,
    version: u32,
    actions: BTreeMap<String, Vec<String>>,
}

impl Default for Bindings {
    fn default() -> Self {
        use KeyCode::*;
        use names::*;
        let k = Button::Key;
        let actions = [
            (MOVE_FORWARD, vec![k(KeyW), k(ArrowUp)]),
            (MOVE_BACK, vec![k(KeyS), k(ArrowDown)]),
            (MOVE_LEFT, vec![k(KeyA), k(ArrowLeft)]),
            (MOVE_RIGHT, vec![k(KeyD), k(ArrowRight)]),
            (RUN, vec![k(ShiftLeft)]),
            (JUMP, vec![k(Space)]),
            (INTERACT, vec![k(KeyE), Button::Mouse(MouseKey::Left)]),
            (PAUSE, vec![k(Escape)]),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_owned(), b))
        .collect();
        Self { actions }
    }
}

impl Bindings {
    pub fn parse(text: &str, path: &str) -> Result<Bindings, BindingsError> {
        let invalid = |message: String| BindingsError::Invalid {
            path: path.to_owned(),
            message,
        };
        let file: BindingsFile = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if file.format != INPUT_FORMAT {
            return Err(invalid(format!("expected \"format\": \"{INPUT_FORMAT}\"")));
        }
        if file.version == 0 || file.version > INPUT_VERSION {
            return Err(invalid(format!(
                "version {} is not supported (this build reads {INPUT_VERSION})",
                file.version
            )));
        }
        let mut actions = BTreeMap::new();
        for (action, buttons) in file.actions {
            if action.is_empty() || action.len() > 64 {
                return Err(invalid("action names must be 1..=64 bytes".into()));
            }
            let parsed = buttons
                .iter()
                .map(|b| {
                    Button::parse(b)
                        .ok_or_else(|| invalid(format!("unknown key `{b}` for action {action}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            actions.insert(action, parsed);
        }
        Ok(Bindings { actions })
    }

    pub fn to_json(&self) -> String {
        let file = BindingsFile {
            format: INPUT_FORMAT.into(),
            version: INPUT_VERSION,
            actions: self
                .actions
                .iter()
                .map(|(a, b)| (a.clone(), b.iter().map(|b| b.name()).collect()))
                .collect(),
        };
        let mut text = serde_json::to_string_pretty(&file).unwrap_or_default();
        text.push('\n');
        text
    }

    /// The project's bindings: `input.json` in the project root if present,
    /// else the defaults. Actions missing from the file keep their defaults.
    pub fn load_project(root: &Path) -> Result<Bindings, BindingsError> {
        let path = root.join(INPUT_FILE);
        let mut bindings = Bindings::default();
        if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|source| BindingsError::Read {
                path: path.display().to_string(),
                source,
            })?;
            bindings.overlay(&Bindings::parse(&text, &path.display().to_string())?);
        }
        Ok(bindings)
    }

    /// Replaces the buttons of every action named in `other`.
    pub fn overlay(&mut self, other: &Bindings) {
        for (a, b) in &other.actions {
            self.actions.insert(a.clone(), b.clone());
        }
    }

    /// Binds `button` to `action` as its only button and removes it from
    /// every other action, so one key never triggers two actions.
    pub fn rebind(&mut self, action: &str, button: Button) {
        for buttons in self.actions.values_mut() {
            buttons.retain(|b| *b != button);
        }
        self.actions.insert(action.to_owned(), vec![button]);
    }

    pub fn buttons(&self, action: &str) -> &[Button] {
        self.actions.get(action).map_or(&[], Vec::as_slice)
    }
}

/// Actions as seen by one simulation tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionState {
    held: BTreeSet<String>,
    pressed: BTreeSet<String>,
    /// Look input in degrees (already scaled by sensitivity, Y inverted if
    /// the player chose that). +x turns right, +y looks up.
    pub look: Vec2,
}

impl ActionState {
    pub fn held(&self, action: &str) -> bool {
        self.held.contains(action)
    }

    /// True only on the tick that first observes the press.
    pub fn pressed(&self, action: &str) -> bool {
        self.pressed.contains(action)
    }

    /// `positive` minus `negative`, each 0 or 1.
    pub fn axis(&self, negative: &str, positive: &str) -> f32 {
        f32::from(u8::from(self.held(positive))) - f32::from(u8::from(self.held(negative)))
    }

    /// Builds a state directly (headless scripts, tests).
    pub fn scripted(held: &[&str], pressed: &[&str], look: Vec2) -> Self {
        let mut s = Self {
            held: held.iter().map(|s| (*s).to_owned()).collect(),
            pressed: pressed.iter().map(|s| (*s).to_owned()).collect(),
            look,
        };
        // A press is also held on its tick.
        let pressed: Vec<String> = s.pressed.iter().cloned().collect();
        s.held.extend(pressed);
        s
    }
}

/// Collects window input between ticks and turns it into [`ActionState`].
#[derive(Debug, Default)]
pub struct ActionInput {
    held: HashSet<Button>,
    pending: HashSet<Button>,
    mouse: Vec2,
}

impl ActionInput {
    pub fn button_down(&mut self, button: Button, repeat: bool) {
        if repeat {
            return;
        }
        if self.held.insert(button) {
            self.pending.insert(button);
        }
    }

    pub fn button_up(&mut self, button: Button) {
        self.held.remove(&button);
    }

    pub fn mouse_moved(&mut self, delta: Vec2) {
        if delta.is_finite() {
            self.mouse += delta;
        }
    }

    /// Releases everything (focus loss, pause, menus).
    pub fn clear(&mut self) {
        self.held.clear();
        self.pending.clear();
        self.mouse = Vec2::ZERO;
    }

    /// Input for the next tick. `degrees_per_count` scales mouse motion.
    pub fn begin_tick(
        &mut self,
        bindings: &Bindings,
        degrees_per_count: f32,
        invert_y: bool,
    ) -> ActionState {
        let pending = std::mem::take(&mut self.pending);
        let mouse = std::mem::take(&mut self.mouse);
        let mut state = ActionState::default();
        for (action, buttons) in &bindings.actions {
            if buttons.iter().any(|b| self.held.contains(b)) {
                state.held.insert(action.clone());
            }
            if buttons.iter().any(|b| pending.contains(b)) {
                state.pressed.insert(action.clone());
                state.held.insert(action.clone());
            }
        }
        let y = if invert_y { mouse.y } else { -mouse.y };
        state.look = Vec2::new(mouse.x, y) * degrees_per_count;
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_round_trip_and_reject_unknown_keys() {
        let b = Bindings::default();
        let text = b.to_json();
        assert_eq!(Bindings::parse(&text, "x").unwrap(), b);
        let bad = text.replace("KeyW", "KeyWW");
        assert!(
            Bindings::parse(&bad, "x")
                .unwrap_err()
                .to_string()
                .contains("KeyWW")
        );
        let future = text.replace("\"version\": 1", "\"version\": 9");
        assert!(Bindings::parse(&future, "x").is_err());
    }

    #[test]
    fn rebinding_moves_a_key_between_actions() {
        let mut b = Bindings::default();
        b.rebind(names::JUMP, Button::Key(KeyCode::KeyE));
        assert_eq!(b.buttons(names::JUMP), &[Button::Key(KeyCode::KeyE)]);
        assert!(
            !b.buttons(names::INTERACT)
                .contains(&Button::Key(KeyCode::KeyE))
        );
    }

    #[test]
    fn presses_reach_exactly_one_tick_and_short_taps_are_kept() {
        let b = Bindings::default();
        let mut input = ActionInput::default();
        input.button_down(Button::Key(KeyCode::KeyE), false);
        input.button_up(Button::Key(KeyCode::KeyE));
        let t1 = input.begin_tick(&b, 0.1, false);
        assert!(t1.pressed(names::INTERACT));
        let t2 = input.begin_tick(&b, 0.1, false);
        assert!(!t2.pressed(names::INTERACT) && !t2.held(names::INTERACT));
        // Repeat events do not create new presses.
        input.button_down(Button::Key(KeyCode::KeyW), false);
        input.button_down(Button::Key(KeyCode::KeyW), true);
        let t3 = input.begin_tick(&b, 0.1, false);
        let t4 = input.begin_tick(&b, 0.1, false);
        assert!(t3.pressed(names::MOVE_FORWARD));
        assert!(!t4.pressed(names::MOVE_FORWARD) && t4.held(names::MOVE_FORWARD));
        assert_eq!(t4.axis(names::MOVE_BACK, names::MOVE_FORWARD), 1.0);
        // Mouse look: screen-down motion looks down unless inverted.
        input.mouse_moved(Vec2::new(10.0, 20.0));
        let t5 = input.begin_tick(&b, 0.1, false);
        assert_eq!(t5.look, Vec2::new(1.0, -2.0));
        input.mouse_moved(Vec2::new(0.0, 20.0));
        assert_eq!(input.begin_tick(&b, 0.1, true).look, Vec2::new(0.0, 2.0));
        input.clear();
        assert!(!input.begin_tick(&b, 0.1, false).held(names::MOVE_FORWARD));
    }
}
