//! Versioned command batches, applied all-or-nothing.
//!
//! ```json
//! {
//!   "format": "psx_command_batch",
//!   "version": 1,
//!   "expected_revision": 3,
//!   "commands": [ { "op": "create_entity", "name": "Crate" }, ... ]
//! }
//! ```
//!
//! The batch runs against a copy of the scene. If any command fails, or the
//! result is invalid, the original is untouched and the error names the
//! failing command's index. On success the copy's revision is incremented.

use copperframe_core::EntityId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::command::{Command, CommandError, apply};
use crate::scene::{Scene, SceneProblem};

pub const BATCH_FORMAT: &str = "psx_command_batch";
pub const BATCH_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandBatch {
    pub format: String,
    pub version: u32,
    /// If set, the batch is rejected unless the scene has this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    pub commands: Vec<Command>,
}

impl CommandBatch {
    pub fn new(commands: Vec<Command>) -> Self {
        Self {
            format: BATCH_FORMAT.to_owned(),
            version: BATCH_VERSION,
            expected_revision: None,
            commands,
        }
    }
}

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("batch is not valid JSON or has invalid commands: {0}")]
    Parse(serde_json::Error),
    #[error("not a command batch (expected \"format\": \"{BATCH_FORMAT}\")")]
    WrongFormat,
    #[error("batch version {0} is not supported (this build reads version {BATCH_VERSION})")]
    UnsupportedVersion(u32),
    #[error(
        "scene revision is {actual} but the batch expects {expected}; \
         inspect the scene again and rebuild the batch"
    )]
    RevisionMismatch { expected: u64, actual: u64 },
    #[error("command {index} ({op}) failed: {error}")]
    Command {
        index: usize,
        op: &'static str,
        error: CommandError,
    },
    #[error("the batch would leave the scene invalid: {}", .0.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "))]
    InvalidResult(Vec<SceneProblem>),
}

/// What one command did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandOutcome {
    pub index: usize,
    pub op: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<EntityId>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchReport {
    pub previous_revision: u64,
    pub revision: u64,
    pub commands: Vec<CommandOutcome>,
}

/// Parses and checks the header of a batch file.
pub fn parse_batch(text: &str) -> Result<CommandBatch, BatchError> {
    let batch: CommandBatch = serde_json::from_str(text).map_err(BatchError::Parse)?;
    if batch.format != BATCH_FORMAT {
        return Err(BatchError::WrongFormat);
    }
    if batch.version == 0 || batch.version > BATCH_VERSION {
        return Err(BatchError::UnsupportedVersion(batch.version));
    }
    Ok(batch)
}

/// Applies every command to a copy of `scene`. Returns the edited copy (with
/// its revision incremented) or the first error; `scene` is never changed.
pub fn apply_batch(
    scene: &Scene,
    batch: &CommandBatch,
) -> Result<(Scene, BatchReport), BatchError> {
    if let Some(expected) = batch.expected_revision
        && expected != scene.revision
    {
        return Err(BatchError::RevisionMismatch {
            expected,
            actual: scene.revision,
        });
    }
    let mut work = scene.clone();
    let mut outcomes = Vec::with_capacity(batch.commands.len());
    for (index, command) in batch.commands.iter().enumerate() {
        let change = apply(&mut work, command).map_err(|error| BatchError::Command {
            index,
            op: command.op(),
            error,
        })?;
        outcomes.push(CommandOutcome {
            index,
            op: command.op(),
            created: change.created,
        });
    }
    let problems = work.problems();
    if !problems.is_empty() {
        return Err(BatchError::InvalidResult(problems));
    }
    work.revision = scene.revision + 1;
    let report = BatchReport {
        previous_revision: scene.revision,
        revision: work.revision,
        commands: outcomes,
    };
    Ok((work, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_all_or_nothing_and_reports_created_ids() {
        let scene = Scene::new("t");
        let room = EntityId::new();
        let text = format!(
            r#"{{
              "format": "psx_command_batch", "version": 1, "expected_revision": 0,
              "commands": [
                {{"op": "create_entity", "id": "{room}", "name": "Room"}},
                {{"op": "create_entity", "name": "Crate", "parent": "{room}",
                  "mesh": {{"source": {{"kind": "primitive", "shape": "cube"}}}}}},
                {{"op": "set_transform", "id": "{room}", "translation": [0, 1, 0]}}
              ]
            }}"#
        );
        let batch = parse_batch(&text).unwrap();
        let (edited, report) = apply_batch(&scene, &batch).unwrap();
        assert_eq!(report.revision, 1);
        assert_eq!(edited.revision, 1);
        assert_eq!(report.commands[0].created, vec![room]);
        let crate_id = report.commands[1].created[0];
        assert_eq!(edited.get(crate_id).unwrap().parent, Some(room));

        // Same batch again: the explicit id is taken, and the revision is stale.
        let err = apply_batch(&edited, &batch).unwrap_err();
        assert!(matches!(
            err,
            BatchError::RevisionMismatch {
                expected: 0,
                actual: 1
            }
        ));
        let mut retry = batch.clone();
        retry.expected_revision = None;
        let err = apply_batch(&edited, &retry).unwrap_err();
        assert!(
            matches!(
                err,
                BatchError::Command {
                    index: 0,
                    error: CommandError::IdInUse(_),
                    ..
                }
            ),
            "{err}"
        );

        // A failure in the last command discards the earlier ones.
        let mut partial = CommandBatch::new(batch.commands[..2].to_vec());
        partial.commands.push(Command::DeleteEntity {
            id: EntityId::new(),
        });
        assert!(matches!(
            apply_batch(&scene, &partial),
            Err(BatchError::Command { index: 2, .. })
        ));
        assert!(scene.entities.is_empty());
    }

    #[test]
    fn header_is_checked() {
        assert!(matches!(
            parse_batch(r#"{"format":"x","version":1,"commands":[]}"#),
            Err(BatchError::WrongFormat)
        ));
        assert!(matches!(
            parse_batch(r#"{"format":"psx_command_batch","version":7,"commands":[]}"#),
            Err(BatchError::UnsupportedVersion(7))
        ));
        assert!(matches!(
            parse_batch(
                r#"{"format":"psx_command_batch","version":1,"commands":[{"op":"explode"}]}"#
            ),
            Err(BatchError::Parse(_))
        ));
    }
}
