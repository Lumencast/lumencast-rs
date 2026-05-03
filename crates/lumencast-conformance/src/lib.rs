//! LSDP/1 conformance harness.
//!
//! Drives an LSDP/1 server through scenarios published in
//! `lumencast-protocol/conformance/v1/scenarios/`. The harness
//! communicates with the server via the LSDP/1 WebSocket endpoint
//! (for client→server frames) and the HTTP test control plane (for
//! priming server-authoritative state, asserting it, and scheduling
//! server-driven deltas).
//!
//! See [`harness::run`] for the entry point.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod control;
pub mod harness;
pub mod local;
pub mod placeholders;
pub mod player;
pub mod scenario;

pub use control::ControlClient;
pub use harness::{Config, Outcome, Report, Target, run};
pub use placeholders::Substitutions;
pub use scenario::{Scenario, Step, Tag};
