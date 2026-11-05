//! The project's default key bindings, `input.json`, as data: what the
//! editor's Project Settings panel reads and writes. The game reads the same
//! file through `copperframe_game::actions::Bindings`, which checks that its
//! key list and default actions match the ones here.
//!
//! ```json
//! { "format": "psx_input", "version": 1,
//!   "actions": { "move_forward": ["KeyW", "ArrowUp"], "interact": ["KeyE", "MouseLeft"] } }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const INPUT_FORMAT: &str = "psx_input";
pub const INPUT_VERSION: u32 = 1;
/// File name of the project's default bindings, in the project root.
pub const INPUT_FILE: &str = "input.json";
/// Longest action name.
pub const MAX_ACTION_LEN: usize = 64;

/// Every button a bindings file may name: the winit `KeyCode` names of the
/// bindable keys, then the mouse buttons.
pub const BUTTON_NAMES: &[&str] = &[
    "KeyA",
    "KeyB",
    "KeyC",
    "KeyD",
    "KeyE",
    "KeyF",
    "KeyG",
    "KeyH",
    "KeyI",
    "KeyJ",
    "KeyK",
    "KeyL",
    "KeyM",
    "KeyN",
    "KeyO",
    "KeyP",
    "KeyQ",
    "KeyR",
    "KeyS",
    "KeyT",
    "KeyU",
    "KeyV",
    "KeyW",
    "KeyX",
    "KeyY",
    "KeyZ",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "CapsLock",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
    "Backslash",
    "Backquote",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Numpad0",
    "Numpad1",
    "Numpad2",
    "Numpad3",
    "Numpad4",
    "Numpad5",
    "Numpad6",
    "Numpad7",
    "Numpad8",
    "Numpad9",
    "NumpadEnter",
    "MouseLeft",
    "MouseRight",
    "MouseMiddle",
];

/// The actions every game has, with their default buttons.
pub const DEFAULT_ACTIONS: &[(&str, &[&str])] = &[
    ("move_forward", &["KeyW", "ArrowUp"]),
    ("move_back", &["KeyS", "ArrowDown"]),
    ("move_left", &["KeyA", "ArrowLeft"]),
    ("move_right", &["KeyD", "ArrowRight"]),
    ("run", &["ShiftLeft"]),
    ("jump", &["Space"]),
    ("interact", &["KeyE", "MouseLeft"]),
    ("pause", &["Escape"]),
];

/// Short label for a button name ("W", "Space", "Mouse Left").
pub fn button_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("Key") {
        return rest.to_owned();
    }
    if let Some(rest) = name.strip_prefix("Digit") {
        return rest.to_owned();
    }
    if let Some(rest) = name.strip_prefix("Mouse") {
        return format!("Mouse {rest}");
    }
    name.to_owned()
}

pub fn is_default_action(name: &str) -> bool {
    DEFAULT_ACTIONS.iter().any(|(a, _)| *a == name)
}

/// Whether `name` can be an action name.
pub fn valid_action_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ACTION_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Action name → button names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputMap {
    pub actions: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFile {
    format: String,
    version: u32,
    actions: BTreeMap<String, Vec<String>>,
}

impl Default for InputMap {
    fn default() -> Self {
        Self {
            actions: DEFAULT_ACTIONS
                .iter()
                .map(|(a, b)| ((*a).to_owned(), b.iter().map(|s| (*s).to_owned()).collect()))
                .collect(),
        }
    }
}

impl InputMap {
    /// Parses a bindings file with the same rules as the game.
    pub fn parse(text: &str) -> Result<InputMap, String> {
        let file: InputFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if file.format != INPUT_FORMAT {
            return Err(format!("expected \"format\": \"{INPUT_FORMAT}\""));
        }
        if file.version == 0 || file.version > INPUT_VERSION {
            return Err(format!(
                "version {} is not supported (this build reads {INPUT_VERSION})",
                file.version
            ));
        }
        let map = InputMap {
            actions: file.actions,
        };
        map.check()?;
        Ok(map)
    }

    /// Checks names the game would reject.
    pub fn check(&self) -> Result<(), String> {
        for (action, buttons) in &self.actions {
            if action.is_empty() || action.len() > MAX_ACTION_LEN {
                return Err(format!("action names must be 1..={MAX_ACTION_LEN} bytes"));
            }
            if let Some(b) = buttons.iter().find(|b| !BUTTON_NAMES.contains(&b.as_str())) {
                return Err(format!("unknown key `{b}` for action {action}"));
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        let file = InputFile {
            format: INPUT_FORMAT.into(),
            version: INPUT_VERSION,
            actions: self.actions.clone(),
        };
        let mut text = serde_json::to_string_pretty(&file).unwrap_or_default();
        text.push('\n');
        text
    }

    pub fn path(root: &Path) -> PathBuf {
        root.join(INPUT_FILE)
    }

    /// The project's bindings as the game sees them: the defaults with
    /// `input.json` (if present) laid over them.
    pub fn load_project(root: &Path) -> Result<InputMap, String> {
        let path = Self::path(root);
        let mut map = InputMap::default();
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let file = Self::parse(&text).map_err(|e| {
                format!("{} is not a valid input bindings file: {e}", path.display())
            })?;
            map.actions.extend(file.actions);
        }
        Ok(map)
    }

    /// Writes every action to the project's `input.json`.
    pub fn save_project(&self, root: &Path) -> Result<(), String> {
        self.check()?;
        let path = Self::path(root);
        copperframe_core::fsutil::atomic_write(&path, self.to_json().as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Adds `button` to `action` and takes it off every other action, so one
    /// key never triggers two actions. Returns the actions it was taken from.
    pub fn bind(&mut self, action: &str, button: &str) -> Vec<String> {
        let mut moved = Vec::new();
        for (a, buttons) in self.actions.iter_mut() {
            if a != action && buttons.iter().any(|b| b == button) {
                buttons.retain(|b| b != button);
                moved.push(a.clone());
            }
        }
        let list = self.actions.entry(action.to_owned()).or_default();
        if !list.iter().any(|b| b == button) {
            list.push(button.to_owned());
        }
        moved
    }

    /// The action `button` is bound to, if any.
    pub fn action_of(&self, button: &str) -> Option<&str> {
        self.actions
            .iter()
            .find(|(_, b)| b.iter().any(|b| b == button))
            .map(|(a, _)| a.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf_input_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn round_trip_and_checks() {
        let map = InputMap::default();
        assert_eq!(InputMap::parse(&map.to_json()).unwrap(), map);
        let bad = map.to_json().replace("KeyW", "KeyWW");
        assert!(InputMap::parse(&bad).unwrap_err().contains("KeyWW"));
        let future = map.to_json().replace("\"version\": 1", "\"version\": 9");
        assert!(InputMap::parse(&future).is_err());
        for (_, buttons) in DEFAULT_ACTIONS {
            for b in *buttons {
                assert!(BUTTON_NAMES.contains(b), "{b}");
            }
        }
    }

    #[test]
    fn bind_moves_a_button_between_actions() {
        let mut map = InputMap::default();
        let moved = map.bind("jump", "KeyE");
        assert_eq!(moved, vec!["interact".to_owned()]);
        assert_eq!(map.actions["jump"], vec!["Space", "KeyE"]);
        assert_eq!(map.actions["interact"], vec!["MouseLeft"]);
        assert_eq!(map.action_of("KeyE"), Some("jump"));
        assert!(map.bind("jump", "KeyE").is_empty(), "already there");
        assert_eq!(map.actions["jump"].len(), 2);
    }

    #[test]
    fn project_file_overlays_defaults() {
        let root = temp("overlay");
        assert_eq!(InputMap::load_project(&root).unwrap(), InputMap::default());
        let mut map = InputMap::default();
        map.actions.insert("crouch".into(), vec!["KeyC".into()]);
        map.actions.insert("jump".into(), vec!["KeyJ".into()]);
        map.save_project(&root).unwrap();
        // A file naming only some actions keeps the others' defaults.
        std::fs::write(
            InputMap::path(&root),
            r#"{"format":"psx_input","version":1,"actions":{"jump":["KeyJ"]}}"#,
        )
        .unwrap();
        let loaded = InputMap::load_project(&root).unwrap();
        assert_eq!(loaded.actions["jump"], vec!["KeyJ"]);
        assert_eq!(loaded.actions["move_forward"], vec!["KeyW", "ArrowUp"]);
        std::fs::write(InputMap::path(&root), "nope").unwrap();
        assert!(InputMap::load_project(&root).is_err());
        assert!(valid_action_name("crouch_2"));
        assert!(!valid_action_name("has space"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
