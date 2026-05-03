//! LSDP/1 conformance harness.
//!
//! The upstream conformance suite lives in `lumencast-protocol/conformance/`
//! as language-agnostic fixtures (byte-level golden frames) and YAML
//! scenarios (end-to-end protocol exchanges). This crate is the Rust
//! driver for those artefacts.
//!
//! As of v0.1 of `lumencast-rs`, the upstream suite is a placeholder
//! manifest with no scenarios published yet, so this crate exposes:
//!
//! - [`local`] — fixtures defined inline that exercise the round-trip
//!   contract every implementation MUST honour. They serve as a
//!   regression baseline until the official suite is fleshed out.
//! - [`harness`] — the `run` API and report types that will execute
//!   the official scenarios once they land.
//!
//! When the upstream suite ships scenarios, this crate will load them
//! via `include_str!` from a git submodule at
//! `external/lumencast-protocol/conformance/v1/`.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod harness;
pub mod local;
