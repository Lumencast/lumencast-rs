//! Typed shapes for every LSDP/1 frame.
//!
//! Each variant of [`ServerFrame`] and [`ClientFrame`] mirrors the wire
//! shape from LSDP/1 §3 and §4. The outer envelope (the `v: 1` field) is
//! handled by [`crate::codec`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ErrorCode;
use crate::types::{Cause, Patch, SceneId, SceneTransition, SceneVersion, SessionId, Token};

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
    /// Optional provenance metadata (LSDP/1.1 §3.2.3). Receivers MUST
    /// NOT use it for semantic decisions — debug/audit only. Omitted
    /// from the wire when `None` for 1.0 wire-shape parity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
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
    /// Previously active scene id (LSDP/1.1 §3.3.1). 1.0 receivers
    /// ignore. Omitted from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_scene_id: Option<SceneId>,
    /// Show-level scene transition (LSDP/1.1 §3.3.1). When absent,
    /// the runtime falls back to its default crossfade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<SceneTransition>,
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

/// LSDP/1 §3.5 — `pong` heartbeat reply.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pong {
    /// Echoes the matching [`Ping::nonce`] verbatim (LSDP/1.1 §3.5).
    /// 1.0 servers reply with a bare pong (`None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
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
    Pong(Pong),
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
            Self::Pong(_) => None,
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
            Self::Pong(_) => "pong",
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
    /// Last seq the client successfully observed before disconnect
    /// (LSDP/1.1 §4.1, §18). Server replies with deltas resuming from
    /// `since_sequence + 1` if the replay buffer covers, else falls back
    /// to a fresh snapshot. 1.0 servers MUST ignore this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_sequence: Option<u64>,
}

/// LSDP/1 §4.2 — `input` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// Non-empty array of patches.
    pub patches: Vec<Patch>,
    /// Free-form correlation tag (LSDP/1.1 §4.2). Server MUST echo
    /// verbatim in `Cause::input_id` of the resulting delta — enables
    /// optimistic-UI reconciliation. 1.0 servers ignore this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_msg_id: Option<String>,
}

/// LSDP/1 §4.3 — `ping` heartbeat.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ping {
    /// Free-form correlation identifier (LSDP/1.1 §4.3). Receiver MUST
    /// echo verbatim in the [`Pong`] reply. 1.0 receivers reply with a
    /// bare pong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

/// LSDP/1.1 §4.4 — `unsubscribe` clean teardown signal. The server MUST
/// close the WebSocket within 1 second of receipt. No data flows after
/// this frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Unsubscribe {}

/// Client → server frames. Internally tagged on `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// `subscribe`.
    Subscribe(Subscribe),
    /// `input`.
    Input(Input),
    /// `ping` heartbeat.
    Ping(Ping),
    /// `unsubscribe` clean teardown (LSDP/1.1 §4.4).
    Unsubscribe(Unsubscribe),
}

impl ClientFrame {
    /// Wire form of the frame discriminator.
    #[must_use]
    pub fn type_str(&self) -> &'static str {
        match self {
            Self::Subscribe(_) => "subscribe",
            Self::Input(_) => "input",
            Self::Ping(_) => "ping",
            Self::Unsubscribe(_) => "unsubscribe",
        }
    }
}
