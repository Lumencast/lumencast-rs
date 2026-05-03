//! [`LeafPath`] — dot-separated leaf-grain path with optional reserved
//! namespace prefix and `{scope}` placeholders.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::errors::{ErrorCode, LumencastError};

/// Reserved top-level namespace tags from LSDP/1 §10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// `__inputs.*` — operator-controllable values.
    Inputs,
    /// `__system.*` — server-emitted system state.
    System,
    /// `__test.*` — test-session sandbox.
    Test,
    /// `__schema.*` — reserved for introspection.
    Schema,
    /// User-defined path (no `__` prefix).
    User,
}

impl Namespace {
    /// Tag inferred from a path string.
    #[must_use]
    pub fn of(path: &str) -> Self {
        if let Some(rest) = path.strip_prefix("__") {
            // First segment up to '.'
            let head = rest.split('.').next().unwrap_or("");
            match head {
                "inputs" => Self::Inputs,
                "system" => Self::System,
                "test" => Self::Test,
                "schema" => Self::Schema,
                _ => Self::User, // unknown reserved — treat as user-illegal
            }
        } else {
            Self::User
        }
    }
}

/// Dot-separated path identifying a leaf in the state tree.
///
/// LSDP/1 leaf paths are sequences of segments separated by `.`. Each
/// segment is one of:
///
/// - An identifier: ASCII alphanumeric or `_`, starting with a non-digit.
///   The very first segment may instead start with `__` (reserved
///   namespace).
/// - A non-negative integer (array index).
/// - A `{scope-name}` placeholder, valid only inside an LSML `repeat`
///   template.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LeafPath(String);

impl TryFrom<String> for LeafPath {
    type Error = LumencastError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        LeafPath::parse(s)
    }
}

impl From<LeafPath> for String {
    fn from(p: LeafPath) -> String {
        p.0
    }
}

impl LeafPath {
    /// Build a `LeafPath` without validation. Prefer
    /// [`LeafPath::parse`] when the input may be untrusted.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Parse and validate a leaf path string.
    pub fn parse(s: impl Into<String>) -> Result<Self, LumencastError> {
        let s = s.into();
        validate(&s)?;
        Ok(Self(s))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert into the underlying string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// Returns the namespace tag derived from the prefix.
    #[must_use]
    pub fn namespace(&self) -> Namespace {
        Namespace::of(&self.0)
    }

    /// Returns `true` if the path starts with `prefix.` or equals
    /// `prefix`. Useful for `service` token scope checks.
    #[must_use]
    pub fn starts_with_prefix(&self, prefix: &str) -> bool {
        if self.0 == prefix {
            return true;
        }
        if let Some(rest) = self.0.strip_prefix(prefix) {
            rest.starts_with('.')
        } else {
            false
        }
    }

    /// Returns `true` if the path contains a `{scope}` placeholder.
    #[must_use]
    pub fn has_scope(&self) -> bool {
        self.0.contains('{')
    }

    /// Substitute every `{scope}` occurrence with `(name, replacement)`
    /// pairs.
    ///
    /// Used by an LSML runtime when expanding `repeat` templates: e.g.
    /// `"{player}.score"` with `[("player", "players.0")]` becomes
    /// `"players.0.score"`.
    #[must_use]
    pub fn substitute(&self, bindings: &[(&str, &str)]) -> Self {
        let mut out = self.0.clone();
        for (name, replacement) in bindings {
            let needle = format!("{{{name}}}");
            out = out.replace(&needle, replacement);
        }
        Self(out)
    }
}

impl fmt::Display for LeafPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for LeafPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for LeafPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

fn validate(s: &str) -> Result<(), LumencastError> {
    if s.is_empty() {
        return Err(LumencastError::invalid_path("empty leaf path"));
    }
    for (i, segment) in s.split('.').enumerate() {
        if segment.is_empty() {
            return Err(LumencastError::invalid_path(
                "empty segment (consecutive dots)",
            ));
        }
        if is_scope_placeholder(segment) {
            // Validate placeholder name.
            let inner = &segment[1..segment.len() - 1];
            if inner.is_empty() || !inner.bytes().all(is_ident_byte) {
                return Err(LumencastError::invalid_path(
                    "scope placeholder must wrap a non-empty identifier",
                ));
            }
            continue;
        }
        if i == 0 && segment.starts_with("__") {
            // Reserved namespace head. The remainder of the segment
            // (after `__`) must be a non-empty identifier.
            let head = &segment[2..];
            if head.is_empty() || !is_identifier(head) {
                return Err(LumencastError::invalid_path(
                    "reserved namespace head is invalid",
                ));
            }
            continue;
        }
        if !is_segment_token(segment) {
            return Err(LumencastError::invalid_path(format!(
                "invalid segment: {segment:?}"
            )));
        }
    }
    Ok(())
}

fn is_scope_placeholder(seg: &str) -> bool {
    seg.starts_with('{') && seg.ends_with('}') && seg.len() >= 2
}

fn is_segment_token(seg: &str) -> bool {
    is_index(seg) || is_identifier(seg)
}

fn is_index(seg: &str) -> bool {
    !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit())
}

fn is_identifier(seg: &str) -> bool {
    let mut bytes = seg.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(is_ident_byte)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl LumencastError {
    pub(crate) fn invalid_path(msg: impl Into<String>) -> Self {
        LumencastError::InvalidValue {
            code: ErrorCode::InvalidValue,
            message: format!("invalid leaf path: {}", msg.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_paths() {
        for s in [
            "show.title",
            "players.0.score",
            "a",
            "_underscore_first",
            "x.0.1.2",
        ] {
            assert!(LeafPath::parse(s).is_ok(), "rejected: {s:?}");
        }
    }

    #[test]
    fn accepts_reserved_namespaces() {
        for s in [
            "__inputs.show_title",
            "__system.health",
            "__test.mock.score",
            "__schema.types",
        ] {
            assert!(LeafPath::parse(s).is_ok(), "rejected: {s:?}");
        }
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", ".", "a..b", "1abc", "show-title", "a.b!"] {
            assert!(LeafPath::parse(s).is_err(), "accepted: {s:?}");
        }
    }

    #[test]
    fn namespace_inference() {
        assert_eq!(Namespace::of("__inputs.x"), Namespace::Inputs);
        assert_eq!(Namespace::of("__system.x"), Namespace::System);
        assert_eq!(Namespace::of("__test.x"), Namespace::Test);
        assert_eq!(Namespace::of("__schema.x"), Namespace::Schema);
        assert_eq!(Namespace::of("show.x"), Namespace::User);
    }

    #[test]
    fn substitute_scopes() {
        let p = LeafPath::parse("{player}.score").unwrap();
        let r = p.substitute(&[("player", "players.0")]);
        assert_eq!(r.as_str(), "players.0.score");
    }

    #[test]
    fn starts_with_prefix() {
        let p = LeafPath::from("players.0.score");
        assert!(p.starts_with_prefix("players"));
        assert!(p.starts_with_prefix("players.0"));
        assert!(!p.starts_with_prefix("playersx"));
        assert!(!p.starts_with_prefix("players.0.scoreboard"));
    }
}
