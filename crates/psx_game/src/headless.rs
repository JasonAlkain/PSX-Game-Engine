//! Headless runs: the simulation without a window, GPU or audio device,
//! driven by an input script, producing a JSON report. Used by `psx
//! test-scene`, CI and automated agents.
//!
//! Script (`psx_script` v1):
//! ```json
//! {
//!   "format": "psx_script", "version": 1,
//!   "steps": [
//!     {"ticks": 60, "hold": ["move_forward"], "look": [0, 0]},
//!     {"ticks": 1, "press": ["interact"]}
//!   ],
//!   "expect": {
//!     "finished": true,
//!     "events": [{"event": "custom", "name": "door_opened"}]
//!   }
//! }
//! ```
//! `press` applies on the step's first tick only; `look` (degrees, +x right,
//! +y up) applies on every tick of the step. Each expected event is a
//! partial object: it matches an event whose fields include all of its
//! fields. The run stops early when the game finishes or quits.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::actions::ActionState;
use crate::sim::{Outcome, Simulation};

pub const SCRIPT_FORMAT: &str = "psx_script";
pub const SCRIPT_VERSION: u32 = 1;
pub const REPORT_FORMAT: &str = "psx_run_report";
/// Upper bound on scripted ticks (about 2.8 hours of game time).
pub const MAX_TICKS: u64 = 600_000;

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default = "one")]
    pub ticks: u64,
    #[serde(default)]
    pub hold: Vec<String>,
    #[serde(default)]
    pub press: Vec<String>,
    #[serde(default)]
    pub look: [f32; 2],
}

fn one() -> u64 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default)]
    pub finished: Option<bool>,
    #[serde(default)]
    pub events: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub expect: Option<Expect>,
}

impl Script {
    pub fn idle(ticks: u64) -> Script {
        Script {
            format: SCRIPT_FORMAT.into(),
            version: SCRIPT_VERSION,
            steps: vec![Step {
                ticks,
                ..Step::default()
            }],
            expect: None,
        }
    }

    pub fn parse(text: &str) -> Result<Script, String> {
        let s: Script = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if s.format != SCRIPT_FORMAT {
            return Err(format!("expected \"format\": \"{SCRIPT_FORMAT}\""));
        }
        if s.version == 0 || s.version > SCRIPT_VERSION {
            return Err(format!(
                "script version {} is not supported (this build reads {SCRIPT_VERSION})",
                s.version
            ));
        }
        let total: u64 = s.steps.iter().map(|st| st.ticks).sum();
        if total > MAX_TICKS {
            return Err(format!(
                "the script runs {total} ticks; the limit is {MAX_TICKS}"
            ));
        }
        for st in &s.steps {
            if !st.look.iter().all(|v| v.is_finite()) {
                return Err("look values must be finite".into());
            }
        }
        Ok(s)
    }
}

/// Runs `script` on a simulation whose scene is already loaded. Returns
/// the report; `report["passed"]` is false when an expectation failed.
pub fn run(sim: &mut Simulation, script: &Script) -> Value {
    let known: Vec<String> = crate::actions::Bindings::default()
        .actions
        .keys()
        .cloned()
        .collect();
    let mut warnings: Vec<String> = Vec::new();
    for st in &script.steps {
        for a in st.hold.iter().chain(&st.press) {
            if !known.contains(a) && !warnings.iter().any(|w| w.contains(a.as_str())) {
                warnings.push(format!(
                    "script uses action `{a}`, which is not a default action (behaviours may still read it)"
                ));
            }
        }
    }
    let mut events = Vec::new();
    'steps: for st in &script.steps {
        let hold: Vec<&str> = st.hold.iter().map(String::as_str).collect();
        let press: Vec<&str> = st.press.iter().map(String::as_str).collect();
        let look = glam::Vec2::from(st.look);
        for i in 0..st.ticks {
            if sim.outcome().is_some() {
                break 'steps;
            }
            let input = if i == 0 {
                ActionState::scripted(&hold, &press, look)
            } else {
                ActionState::scripted(&hold, &[], look)
            };
            sim.tick(&input);
            events.extend(sim.take_events());
        }
    }
    warnings.extend(sim.take_warnings());

    let events_json: Vec<Value> = events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
        .collect();
    let mut failures = Vec::new();
    if let Some(expect) = &script.expect {
        if let Some(f) = expect.finished {
            let finished = matches!(sim.outcome(), Some(Outcome::Finished { .. }));
            if f != finished {
                failures.push(if f {
                    "expected the game to finish, but it did not".to_owned()
                } else {
                    "expected the game not to finish, but it did".to_owned()
                });
            }
        }
        for want in &expect.events {
            if !events_json.iter().any(|e| subset(want, e)) {
                failures.push(format!("expected event {want} did not happen"));
            }
        }
    }
    let cam = sim.camera(1.0);
    json!({
        "format": REPORT_FORMAT,
        "version": 1,
        "game": sim.title(),
        "scene": sim.scene_path().map(|p| sim.project().relative(p)),
        "ticks": sim.tick_count(),
        "seconds": sim.tick_count() as f64 / f64::from(crate::sim::TICK_HZ),
        "outcome": sim.outcome(),
        "events": events_json,
        "warnings": warnings,
        "camera": {"position": cam.position.to_array(), "yaw_deg": cam.yaw.to_degrees(), "pitch_deg": cam.pitch.to_degrees()},
        "entities": sim.entity_count(),
        "bodies": sim.body_count(),
        "passed": failures.is_empty(),
        "failures": failures,
    })
}

/// Whether every field of `want` is present (recursively) in `have`.
fn subset(want: &Value, have: &Value) -> bool {
    match (want, have) {
        (Value::Object(w), Value::Object(h)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| subset(v, hv))),
        _ => want == have,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_validated() {
        assert!(
            Script::parse(
                r#"{"format":"psx_script","version":1,"steps":[{"ticks":3,"hold":["jump"]}]}"#
            )
            .is_ok()
        );
        assert!(Script::parse(r#"{"format":"x","version":1}"#).is_err());
        assert!(Script::parse(r#"{"format":"psx_script","version":9}"#).is_err());
        assert!(
            Script::parse(r#"{"format":"psx_script","version":1,"steps":[{"tick":3}]}"#).is_err()
        );
        assert!(
            Script::parse(r#"{"format":"psx_script","version":1,"steps":[{"ticks":999999999}]}"#)
                .is_err()
        );
    }

    #[test]
    fn partial_event_matching() {
        let e = json!({"tick": 4, "event": "custom", "name": "door_opened", "detail": ""});
        assert!(subset(
            &json!({"event": "custom", "name": "door_opened"}),
            &e
        ));
        assert!(!subset(&json!({"event": "custom", "name": "other"}), &e));
    }
}
