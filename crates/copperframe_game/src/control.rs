//! Editor control of a running game (`--control stdin`): pause, resume,
//! single-step and quit, using the line protocol in
//! [`copperframe_assets::play_control`].
//!
//! A background thread reads stdin and hands complete lines to the
//! player's event loop; [`Pacing`] decides how many fixed ticks a frame
//! may run. Steps use the same [`crate::Simulation::tick`] as normal play,
//! so a game stepped N times is in the same state as one that ran N ticks.

use std::io::BufRead;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use copperframe_assets::play_control::{ControlCommand, PlayStatus, RunState};

/// Most ticks a step may run in one frame (the same cap as
/// [`copperframe_core::FixedStep`]), so a long step keeps drawing.
pub const STEP_TICKS_PER_FRAME: u32 = 8;

/// Something read from the control input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Command(ControlCommand),
    /// A line that is not a command; reported back, otherwise ignored.
    Invalid(String),
    /// The input ended: the editor closed the pipe or exited.
    Closed,
}

/// Whether the simulation may advance, and by how many ticks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pacing {
    paused: bool,
    steps: u32,
}

impl Pacing {
    /// Applies pause, resume or step. `Quit` is the caller's job and
    /// changes nothing here.
    pub fn apply(&mut self, command: ControlCommand) {
        match command {
            ControlCommand::Pause => self.paused = true,
            ControlCommand::Resume => {
                self.paused = false;
                self.steps = 0;
            }
            ControlCommand::Step(n) => {
                self.paused = true;
                self.steps = self.steps.saturating_add(n);
            }
            ControlCommand::Quit => {}
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Ticks of a step still to run.
    pub fn pending_steps(&self) -> u32 {
        self.steps
    }

    /// While paused: the step ticks to run this frame (at most
    /// [`STEP_TICKS_PER_FRAME`]), removed from the pending count.
    pub fn take_step_ticks(&mut self) -> u32 {
        let n = self.steps.min(STEP_TICKS_PER_FRAME);
        self.steps -= n;
        n
    }

    /// Drops steps that cannot run (the game finished or failed).
    pub fn cancel_steps(&mut self) {
        self.steps = 0;
    }

    pub fn state(&self) -> RunState {
        match (self.paused, self.steps) {
            (false, _) => RunState::Running,
            (true, 0) => RunState::Paused,
            (true, _) => RunState::Stepping,
        }
    }

    pub fn status(&self, tick: u64) -> PlayStatus {
        PlayStatus::new(self.state(), tick)
    }
}

/// Lines from the editor, read on a background thread.
pub struct ControlInput {
    rx: Receiver<Incoming>,
    closed: bool,
}

impl ControlInput {
    /// Reads the process's stdin.
    pub fn stdin() -> Self {
        Self::from_reader(std::io::BufReader::new(std::io::stdin()))
    }

    /// Reads `reader` line by line until it ends or fails.
    pub fn from_reader(reader: impl BufRead + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("editor-control".into())
            .spawn(move || {
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let item = match ControlCommand::parse(&line) {
                        Ok(Some(c)) => Incoming::Command(c),
                        Ok(None) => continue,
                        Err(e) => Incoming::Invalid(e),
                    };
                    if tx.send(item).is_err() {
                        return;
                    }
                }
                let _ = tx.send(Incoming::Closed);
            });
        if let Err(e) = spawned {
            tracing::error!("could not read editor commands: {e}");
        }
        Self { rx, closed: false }
    }

    /// Everything received since the last call, in order. Reports
    /// [`Incoming::Closed`] once.
    pub fn drain(&mut self) -> Vec<Incoming> {
        let mut out = Vec::new();
        if self.closed {
            return out;
        }
        loop {
            match self.rx.try_recv() {
                Ok(item) => {
                    let end = item == Incoming::Closed;
                    out.push(item);
                    if end {
                        self.closed = true;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The reader thread could not start or ended without
                    // saying so; treat it like a closed pipe.
                    out.push(Incoming::Closed);
                    self.closed = true;
                    break;
                }
            }
        }
        out
    }
}

/// Writes a status line for the editor to stdout.
pub fn report(status: &PlayStatus) {
    println!("{}", status.to_line());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn pacing_pauses_steps_and_resumes() {
        let mut p = Pacing::default();
        assert_eq!(p.state(), RunState::Running);
        p.apply(ControlCommand::Step(20));
        assert_eq!(p.state(), RunState::Stepping);
        assert_eq!(p.take_step_ticks(), STEP_TICKS_PER_FRAME);
        assert_eq!(p.take_step_ticks(), STEP_TICKS_PER_FRAME);
        assert_eq!(p.take_step_ticks(), 4);
        assert_eq!(p.take_step_ticks(), 0);
        assert_eq!(p.state(), RunState::Paused);
        p.apply(ControlCommand::Step(1));
        p.apply(ControlCommand::Step(2));
        assert_eq!(p.pending_steps(), 3);
        p.apply(ControlCommand::Resume);
        assert_eq!(p.state(), RunState::Running);
        assert_eq!(p.pending_steps(), 0);
        p.apply(ControlCommand::Pause);
        p.apply(ControlCommand::Quit);
        assert_eq!(p.status(7), PlayStatus::new(RunState::Paused, 7));
    }

    fn drain_until_closed(input: &mut ControlInput) -> Vec<Incoming> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut all = Vec::new();
        while !all.contains(&Incoming::Closed) && Instant::now() < deadline {
            all.extend(input.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn input_reports_commands_errors_and_the_end() {
        let text = "pause\n\nstep 3\nfly\nresume\n";
        let mut input = ControlInput::from_reader(std::io::Cursor::new(text));
        let all = drain_until_closed(&mut input);
        assert_eq!(all.len(), 5, "{all:?}");
        assert_eq!(all[0], Incoming::Command(ControlCommand::Pause));
        assert_eq!(all[1], Incoming::Command(ControlCommand::Step(3)));
        assert!(matches!(&all[2], Incoming::Invalid(e) if e.contains("fly")));
        assert_eq!(all[3], Incoming::Command(ControlCommand::Resume));
        assert_eq!(all[4], Incoming::Closed);
        assert!(input.drain().is_empty(), "closed is reported once");
    }
}
