//! Pure protocol layer for [LSDP/1].
//!
//! This crate has no IO and no async. It compiles to
//! `wasm32-unknown-unknown` and is suitable for use in browsers, edge
//! runtimes, and embedded contexts.
//!
//! [LSDP/1]: https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSDP-1.md
//!
//! # Modules
//!
//! - [`envelope`]: outer envelope with the protocol version field.
//! - [`frames`]: typed shapes for every LSDP/1 frame.
//! - [`codec`]: JSON encode/decode for whole frames.
//! - [`sequence`]: server-side allocator and client-side gap detector.
//! - [`leaf_path`]: [`LeafPath`] newtype with parsing and scope
//!   substitution.
//! - [`errors`]: closed [`ErrorCode`] taxonomy and crate error type.
//! - [`types`]: shared primitives ([`Patch`], [`SceneId`], …).
//!
//! # Example
//!
//! ```
//! use lumencast_protocol::codec;
//! use lumencast_protocol::frames::{ServerFrame, Snapshot};
//! use lumencast_protocol::types::{SceneId, SceneVersion};
//! use serde_json::json;
//!
//! let frame = ServerFrame::Snapshot(Snapshot {
//!     seq: 1,
//!     scene_id: SceneId::from("main-stage"),
//!     scene_version: SceneVersion::from(
//!         "sha256:0000000000000000000000000000000000000000000000000000000000000000",
//!     ),
//!     state: [("show.title".to_string(), json!("Hello"))]
//!         .into_iter()
//!         .collect(),
//!     ts: None,
//! });
//!
//! let bytes = codec::encode_server(&frame).unwrap();
//! let parsed = codec::decode_server(&bytes).unwrap();
//! assert_eq!(frame, parsed);
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod bundle;
pub mod codec;
pub mod envelope;
pub mod errors;
pub mod frames;
pub mod leaf_path;
pub mod sequence;
pub mod types;

pub use bundle::{Bundle, BundleError, OperatorInput};
pub use envelope::PROTOCOL_VERSION;
pub use errors::{ErrorCode, LumencastError};
pub use frames::{ClientFrame, ServerFrame};
pub use leaf_path::{LeafPath, Namespace};
pub use sequence::{SequenceAllocator, SequenceTracker};
pub use types::{Patch, SceneId, SceneVersion, SessionId, Token};
