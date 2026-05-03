//! Shared primitive types used across the protocol surface.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::leaf_path::LeafPath;

/// Opaque authentication token, sent in `subscribe`.
///
/// LSDP is token-agnostic — the bytes inside are validated by the
/// configured authenticator on the server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(pub String);

impl Token {
    /// Wrap a string as a `Token`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for Token {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Tokens are sensitive — never print contents.
        f.write_str("Token(<redacted>)")
    }
}

/// Stable identifier for a scene.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SceneId(pub String);

impl SceneId {
    /// Wrap a string as a `SceneId`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for SceneId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for SceneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content hash of an LSML bundle, prefixed with the algorithm.
///
/// LSML 1.0 mandates `sha256:` followed by 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SceneVersion(pub String);

impl SceneVersion {
    /// Wrap a string as a `SceneVersion`. The format is **not** validated
    /// here — call [`SceneVersion::is_well_formed`] to check.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if the value matches `sha256:<64 hex>`.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        let Some(rest) = self.0.strip_prefix("sha256:") else {
            return false;
        };
        rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

impl<S: Into<String>> From<S> for SceneVersion {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for SceneVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier for a test session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    /// Wrap a string as a `SessionId`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl<S: Into<String>> From<S> for SessionId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A single `(path, value)` patch carried inside `delta` and `input`
/// frames.
///
/// LSDP/1 forbids JSON objects at the top level of `value`. Use
/// [`Patch::is_value_legal`] to check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    /// Target leaf path.
    pub path: LeafPath,
    /// Value to assign at `path`. MUST NOT be a JSON object — string,
    /// number, boolean, null, or array only (LSDP/1 §3.2).
    pub value: Value,
    /// Per-leaf animation directive (LSDP/1.1 §3.2.2). Servers MAY
    /// emit ; runtimes interpret when applying the new value. 1.0
    /// receivers ignore. Omitted from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<TransitionSpec>,
}

/// Per-leaf animation directive on a delta patch (LSDP/1.1 §3.2.2).
/// Servers MAY emit ; runtimes interpret when applying the new value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransitionSpec {
    /// Linear interpolation over `duration_ms`, optionally eased.
    Tween {
        /// Duration in milliseconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
        /// Easing curve. Defaults to `linear` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        easing: Option<Easing>,
    },
    /// Damped harmonic oscillator.
    Spring {
        /// Stiffness coefficient.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stiffness: Option<f64>,
        /// Damping coefficient.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        damping: Option<f64>,
    },
    /// No animation — assign the new value immediately.
    Snap,
}

/// Easing curve on a `TransitionSpec::Tween`. Strict closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Easing {
    /// Constant rate of change.
    Linear,
    /// Accelerates from rest.
    EaseIn,
    /// Decelerates to rest.
    EaseOut,
    /// Accelerates then decelerates.
    EaseInOut,
}

/// Optional provenance metadata on a delta (LSDP/1.1 §3.2.3). Receivers
/// MUST NOT use it for semantic decisions — debug/audit only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cause {
    /// Free-form origin tag, e.g. `"operator:user-abc"`,
    /// `"adapter:http_poll"`, `"service:ranker"`.
    pub source: String,
    /// Echoes [`Input::client_msg_id`](crate::frames::Input) verbatim when the delta was
    /// caused by an operator input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
}

/// Show-level scene-swap transition on a `scene_changed` frame
/// (LSDP/1.1 §3.3.1). Runtimes that don't recognise `kind` fall back
/// to crossfade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneTransition {
    /// Transition kind. `"crossfade"` is the only standard value in
    /// 1.1 ; vendor-prefixed `x-vendor.*` kinds are valid wire shapes.
    pub kind: String,
    /// Duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
}

impl Patch {
    /// Build a patch without a transition directive.
    pub fn new(path: impl Into<LeafPath>, value: Value) -> Self {
        Self {
            path: path.into(),
            value,
            transition: None,
        }
    }

    /// Build a patch carrying a 1.1 transition directive.
    pub fn with_transition(
        path: impl Into<LeafPath>,
        value: Value,
        transition: TransitionSpec,
    ) -> Self {
        Self {
            path: path.into(),
            value,
            transition: Some(transition),
        }
    }

    /// Returns `true` if the `value` is a string, number, boolean, null,
    /// or array (LSDP/1 §3.2 forbids objects at the top level).
    #[must_use]
    pub fn is_value_legal(&self) -> bool {
        !self.value.is_object()
    }
}

/// Returns `Ok(())` if `value` is a legal LSDP/1 leaf value (not an
/// object), otherwise an `Err`.
pub fn check_leaf_value(value: &Value) -> Result<(), &'static str> {
    if value.is_object() {
        Err("leaf value MUST NOT be a JSON object")
    } else {
        Ok(())
    }
}
