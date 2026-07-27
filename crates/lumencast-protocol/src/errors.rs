//! Closed [`ErrorCode`] taxonomy from LSDP/1 and the crate-level
//! [`LumencastError`] type.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// LSDP/1 error code (closed taxonomy from `ERROR-CODES.md`).
///
/// Codes are stable identifiers — runtimes match by exact string
/// equality. New codes require an LSDP minor version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// `AUTH_DENIED` — token invalid, expired or revoked. Not recoverable.
    AuthDenied,
    /// `WRITE_FORBIDDEN` — role does not allow writing this path.
    WriteForbidden,
    /// `SCENE_NOT_FOUND` — `subscribe.scene` references an unknown scene.
    SceneNotFound,
    /// `BUNDLE_FETCH_FAILED` — runtime cannot retrieve the bundle.
    BundleFetchFailed,
    /// `BUNDLE_INCOMPATIBLE` — bundle major version exceeds runtime.
    BundleIncompatible,
    /// `VERSION_GAP` — runtime detected a missing sequence number.
    VersionGap,
    /// `VERSION_MISMATCH` — protocol major version negotiation failed.
    VersionMismatch,
    /// `UNKNOWN_PATH` — input references undeclared path.
    UnknownPath,
    /// `INVALID_VALUE` — value violates declared type or constraints.
    InvalidValue,
    /// `RATE_LIMIT` — connection exceeded server-side rate limit.
    RateLimit,
    /// `TEST_SESSION_EXPIRED` — test session TTL expired.
    TestSessionExpired,
    /// `INTERNAL` — server-side error not covered by a more specific code.
    Internal,
}

impl ErrorCode {
    /// Recoverability semantics fixed by the spec for each code.
    ///
    /// `INTERNAL` is special — its recoverability is set per-emission by
    /// the server, so this method returns `false` (the safer default);
    /// callers SHOULD honour the actual `recoverable` field on the
    /// outgoing frame instead of relying on this for `INTERNAL`.
    #[must_use]
    pub fn default_recoverable(self) -> bool {
        match self {
            Self::WriteForbidden
            | Self::BundleFetchFailed
            | Self::VersionGap
            | Self::UnknownPath
            | Self::InvalidValue
            | Self::RateLimit => true,
            Self::AuthDenied
            | Self::SceneNotFound
            | Self::BundleIncompatible
            | Self::VersionMismatch
            | Self::TestSessionExpired
            | Self::Internal => false,
        }
    }

    /// Wire form (the exact string the protocol uses).
    #[must_use]
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::AuthDenied => "AUTH_DENIED",
            Self::WriteForbidden => "WRITE_FORBIDDEN",
            Self::SceneNotFound => "SCENE_NOT_FOUND",
            Self::BundleFetchFailed => "BUNDLE_FETCH_FAILED",
            Self::BundleIncompatible => "BUNDLE_INCOMPATIBLE",
            Self::VersionGap => "VERSION_GAP",
            Self::VersionMismatch => "VERSION_MISMATCH",
            Self::UnknownPath => "UNKNOWN_PATH",
            Self::InvalidValue => "INVALID_VALUE",
            Self::RateLimit => "RATE_LIMIT",
            Self::TestSessionExpired => "TEST_SESSION_EXPIRED",
            Self::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

/// Errors raised by the protocol crate.
///
/// These wrap protocol-level failures detected during encode/decode or
/// while validating frames. They are distinct from the [`ErrorCode`]
/// taxonomy carried inside `error` frames on the wire.
#[derive(Debug, Error)]
pub enum LumencastError {
    /// JSON parse failure.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// Envelope is missing required fields or has wrong types.
    #[error("invalid envelope: {0}")]
    InvalidEnvelope(String),

    /// Envelope `v` is not the expected protocol major version.
    #[error("version mismatch: expected v=1, got v={got}")]
    VersionMismatch {
        /// Version received on the wire.
        got: u64,
    },

    /// Server emitted a frame whose `seq` is not contiguous with the
    /// previous one. Held by [`SequenceTracker`](crate::SequenceTracker).
    #[error("sequence gap: expected seq={expected}, got seq={got}")]
    SequenceGap {
        /// Expected sequence number.
        expected: u64,
        /// Sequence number actually received.
        got: u64,
    },

    /// A patch carries an illegal value, or a path is malformed (LSDP/1
    /// §3.2 forbids JSON objects in `value`; §10 constrains paths).
    #[error("invalid value: {message}")]
    InvalidValue {
        /// Error code from the closed taxonomy.
        code: ErrorCode,
        /// Human-readable description.
        message: String,
        /// Offending leaf path, when the rejection is attributable to
        /// one. LSDP/1 §3.4.1 makes `path` REQUIRED on the emitted
        /// `INVALID_VALUE` error frame, so the rejection site — the only
        /// place that knows which patch failed — carries it structurally
        /// instead of only interpolating it into `message`.
        path: Option<String>,
    },
}

impl LumencastError {
    /// Build an `InvalidValue` variant with code `INVALID_VALUE`.
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue {
            code: ErrorCode::InvalidValue,
            message: message.into(),
            path: None,
        }
    }

    /// Build an `InvalidValue` variant that names the offending leaf
    /// path (LSDP/1 §3.4.1).
    pub fn invalid_value_at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidValue {
            code: ErrorCode::InvalidValue,
            message: message.into(),
            path: Some(path.into()),
        }
    }
}
