//! Gameplay runtime for PSX Engine.
//!
//! - [`api`]: the game-facing API ([`Behavior`], [`Game`], [`Registry`]).
//! - [`sim`]: the runtime world built from a scene and advanced in fixed
//!   ticks; runs headless (no window, GPU or audio device needed).
//! - [`actions`]: named input actions and bindings files.
//! - [`physics`]: rapier3d integration (bodies, character controller,
//!   sensors, ray casts).
//! - [`audio`]: kira integration (buses, spatial emitters, listener).
//! - [`builtin`]: engine behaviours (`psx.player`, `psx.camera_zone`,
//!   `psx.spin`).

pub mod actions;
pub mod api;
pub mod audio;
pub mod builtin;
pub mod physics;
pub mod sim;

pub use api::{
    Behavior, BehaviorDescriptor, Entity, FieldDescriptor, FieldKind, Game, GameEvent, Params,
    Registry, TimedEvent,
};
pub use sim::{CameraState, Ctx, Outcome, Simulation, TICK_DT, TICK_HZ};
