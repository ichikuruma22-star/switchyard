//! Durable, permission-reviewed local agent execution through the gateway.
//!
//! UI, CLI, and desktop adapters share this engine. The model cannot approve
//! its own operations; only a person, or a project/session setting a person
//! opted into, can. A shell command is approved local execution, not an
//! operating-system sandbox. Incomplete side effects are never auto-replayed.

#![forbid(unsafe_code)]

mod engine;
mod gateway;
mod store;
mod types;

pub use engine::AppEngine;
pub use types::*;
