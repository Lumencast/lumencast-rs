//! Typed shapes for every LSDP/1 frame.
//!
//! Each variant of [`ServerFrame`] and [`ClientFrame`] mirrors the wire
//! shape from LSDP/1 §3 and §4. The outer envelope (the `v: 1` field) is
//! handled by [`crate::codec`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ErrorCode;
use crate::types::{Patch, SceneId, SceneVersion, SessionId, Token};

/// `state` map carried in `snapshot` frames.
///
/// A flat dictionary `path → JSON value`. We use `BTreeMap` for stable
/// iteration order — useful for tests and conformance fixtures.
pub type State = BTreeMap<String, Value>;

/// LSDP/1 §3.1 — `snapshot` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Sequence number. `1` for the first snapshot of a subscription;
    /// resets to `1` after `scene_changed`.
    pub seq: u64,
    /// Active scene identifier.
    pub scene_id: SceneId,
    /// Content hash of the LSML bundle.
    pub scene_version: SceneVersion,
    /// Full state at this point in time.
    pub state: State,
    /// Optional server timestamp (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// LSDP/1 §3.2 — `delta` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Non-empty array of patches, applied left-to-right atomically.
    pub patches: Vec<Patch>,
    /// Optional server timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// LSDP/1 §3.3 — `scene_changed` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneChanged {
    /// Sequence number on the **outgoing** connection. The next
    /// `snapshot` resets seq to `1`.
    pub seq: u64,
    /// Identifier of the new active scene.
    pub scene_id: SceneId,
    /// Content hash of the new bundle.
    pub scene_version: SceneVersion,
    /// Optional server timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// LSDP/1 §3.4 — `error` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorFrame {
    /// Sequence number.
    pub seq: u64,
    /// Error code from the closed taxonomy.
    pub code: ErrorCode,
    /// Human-readable description (English).
    pub message: String,
    /// Whether the runtime can attempt to continue.
    pub recoverable: bool,
    /// Optional retry hint, set by the server for `RATE_LIMIT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Offending leaf path. Set by the server for `UNKNOWN_PATH` and
    /// `INVALID_VALUE` so the harness can localise authoring errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional server timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// Server → client frames. Internally tagged on `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// `snapshot`.
    Snapshot(Snapshot),
    /// `delta`.
    Delta(Delta),
    /// `scene_changed`.
    SceneChanged(SceneChanged),
    /// `error`.
    Error(ErrorFrame),
    /// `pong` heartbeat reply.
    Pong,
}

impl ServerFrame {
    /// Sequence number of this frame, if any. `pong` has none.
    #[must_use]
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Snapshot(f) => Some(f.seq),
            Self::Delta(f) => Some(f.seq),
            Self::SceneChanged(f) => Some(f.seq),
            Self::Error(f) => Some(f.seq),
            Self::Pong => None,
        }
    }

    /// Wire form of the frame discriminator.
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "snapshot",
            Self::Delta(_) => "delta",
            Self::SceneChanged(_) => "scene_changed",
            Self::Error(_) => "error",
            Self::Pong => "pong",
        }
    }
}

/// LSDP/1 §4.1 — `subscribe` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscribe {
    /// Opaque authentication token.
    pub token: Token,
    /// Optional scene id (required only in test mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
    /// Optional test session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
}

/// LSDP/1 §4.2 — `input` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// Non-empty array of patches.
    pub patches: Vec<Patch>,
}

/// Client → server frames. Internally tagged on `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// `subscribe`.
    Subscribe(Subscribe),
    /// `input`.
    Input(Input),
    /// `ping` heartbeat.
    Ping,
}

impl ClientFrame {
    /// Wire form of the frame discriminator.
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Subscribe(_) => "subscribe",
            Self::Input(_) => "input",
            Self::Ping => "ping",
        }
    }
}
