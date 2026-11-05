//! The line protocol between the editor and a game started for Play with
//! `--control stdin`.
//!
//! The editor writes one command per line to the game's stdin:
//!
//! | Line | Effect |
//! | --- | --- |
//! | `pause` | Stop advancing the simulation (the window keeps drawing) |
//! | `resume` | Continue at normal speed |
//! | `step [n]` | Pause if needed, then advance exactly `n` fixed ticks (default 1, at most [`MAX_STEP`]) |
//! | `quit` | Close the game normally (scene unload and `Game::shutdown` run) |
//!
//! End of input (the editor closed the pipe or exited) also quits, so a
//! game started by an editor that crashed does not keep running.
//!
//! The game answers on stdout with lines that start with
//! [`STATUS_PREFIX`] followed by a [`PlayStatus`] JSON object, after
//! every command and when a step finishes. Other stdout and stderr lines
//! are ordinary log text.
//!
//! This module has no window or GPU dependency so the editor (which never
//! links `copperframe_game`) and the game share one definition.

use serde::{Deserialize, Serialize};

/// Start of every status line the game writes to stdout.
pub const STATUS_PREFIX: &str = "@copperframe ";
/// Format tag of [`PlayStatus`].
pub const STATUS_FORMAT: &str = "copperframe_play_status";
/// Version of [`PlayStatus`].
pub const STATUS_VERSION: u32 = 1;
/// The largest `step` count accepted in one command (10 s at 60 Hz).
pub const MAX_STEP: u32 = 600;

/// A command from the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Pause,
    Resume,
    /// Advance this many ticks, then stay paused.
    Step(u32),
    Quit,
}

impl ControlCommand {
    /// Parses one line. Surrounding whitespace and case are ignored;
    /// blank lines are `Ok(None)`.
    pub fn parse(line: &str) -> Result<Option<Self>, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let mut words = line.split_whitespace();
        let word = words.next().unwrap_or_default().to_ascii_lowercase();
        let arg = words.next();
        if words.next().is_some() {
            return Err(format!("too many words in `{line}`"));
        }
        let no_arg = |c: Self| match arg {
            None => Ok(Some(c)),
            Some(a) => Err(format!("`{word}` takes no argument, found `{a}`")),
        };
        match word.as_str() {
            "pause" => no_arg(Self::Pause),
            "resume" => no_arg(Self::Resume),
            "quit" => no_arg(Self::Quit),
            "step" => {
                let n = match arg {
                    None => 1,
                    Some(a) => a
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=MAX_STEP).contains(n))
                        .ok_or_else(|| {
                            format!(
                                "step count must be a whole number in 1..={MAX_STEP}, found `{a}`"
                            )
                        })?,
                };
                Ok(Some(Self::Step(n)))
            }
            _ => Err(format!(
                "unknown command `{word}` (expected pause, resume, step [n] or quit)"
            )),
        }
    }

    /// The line the editor writes (without the newline).
    pub fn to_line(self) -> String {
        match self {
            Self::Pause => "pause".into(),
            Self::Resume => "resume".into(),
            Self::Step(1) => "step".into(),
            Self::Step(n) => format!("step {n}"),
            Self::Quit => "quit".into(),
        }
    }
}

/// Whether the simulation is advancing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Paused,
    /// Paused, with ticks of a `step` still to run.
    Stepping,
    /// The game is closing.
    Quitting,
}

/// What the game reports after a command or a finished step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayStatus {
    pub format: String,
    pub version: u32,
    pub state: RunState,
    /// Fixed ticks simulated since the game started.
    pub tick: u64,
    /// Set when the last command could not be understood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PlayStatus {
    pub fn new(state: RunState, tick: u64) -> Self {
        Self {
            format: STATUS_FORMAT.into(),
            version: STATUS_VERSION,
            state,
            tick,
            error: None,
        }
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// The stdout line (without the newline).
    pub fn to_line(&self) -> String {
        format!(
            "{STATUS_PREFIX}{}",
            serde_json::to_string(self).unwrap_or_default()
        )
    }

    /// Reads a status line. `None` for ordinary log lines and for status
    /// lines of another format or a newer version.
    pub fn parse_line(line: &str) -> Option<Self> {
        let json = line.trim_end().strip_prefix(STATUS_PREFIX)?;
        let status: Self = serde_json::from_str(json).ok()?;
        (status.format == STATUS_FORMAT && status.version <= STATUS_VERSION).then_some(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_and_round_trip() {
        for c in [
            ControlCommand::Pause,
            ControlCommand::Resume,
            ControlCommand::Step(1),
            ControlCommand::Step(30),
            ControlCommand::Quit,
        ] {
            assert_eq!(ControlCommand::parse(&c.to_line()), Ok(Some(c)));
        }
        assert_eq!(
            ControlCommand::parse("  STEP  5 \r"),
            Ok(Some(ControlCommand::Step(5)))
        );
        assert_eq!(ControlCommand::parse(" \n"), Ok(None));
        for bad in [
            "step 0",
            "step -1",
            "step 601",
            "step x",
            "pause now",
            "jump",
            "step 1 2",
        ] {
            assert!(ControlCommand::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_lines_round_trip_and_ignore_log_text() {
        let s = PlayStatus::new(RunState::Paused, 42);
        assert_eq!(PlayStatus::parse_line(&s.to_line()), Some(s.clone()));
        let e = s.with_error("unknown command `x`");
        assert_eq!(
            PlayStatus::parse_line(&format!("{}\r\n", e.to_line())),
            Some(e)
        );
        assert_eq!(PlayStatus::parse_line("INFO game closed"), None);
        assert_eq!(PlayStatus::parse_line("@copperframe {not json"), None);
        let newer = r#"@copperframe {"format":"copperframe_play_status","version":99,"state":"paused","tick":1}"#;
        assert_eq!(PlayStatus::parse_line(newer), None);
    }
}
