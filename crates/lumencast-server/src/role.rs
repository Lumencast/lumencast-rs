//! Role enforcement (LSDP/1 §9).

use std::fmt;

use lumencast_protocol::LeafPath;
use serde::{Deserialize, Serialize};

/// Role assigned to a connection by the [`crate::Authenticator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only. Cannot send `input`.
    Viewer,
    /// Can write `__inputs.*`.
    Operator,
    /// Can write `__inputs.*`, optionally scoped via [`Identity::paths`](crate::Identity::paths).
    Service,
    /// Test session — can write `__test.*`.
    Test,
}

impl Role {
    /// Returns `true` if this role is allowed to send any `input` frames.
    #[must_use]
    pub fn can_input(self) -> bool {
        !matches!(self, Self::Viewer)
    }

    /// Returns `true` if `path` is writable by this role, given the
    /// optional `paths` claim attached to the identity.
    #[must_use]
    pub fn can_write(self, path: &LeafPath, scoped: Option<&[String]>) -> bool {
        match self {
            Self::Viewer => false,
            Self::Operator => path.starts_with_prefix("__inputs"),
            Self::Service => {
                if !path.starts_with_prefix("__inputs") {
                    return false;
                }
                match scoped {
                    Some(prefixes) if !prefixes.is_empty() => {
                        prefixes.iter().any(|p| path.starts_with_prefix(p))
                    }
                    _ => true,
                }
            }
            Self::Test => path.starts_with_prefix("__test"),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Service => "service",
            Self::Test => "test",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_cannot_write() {
        let p = LeafPath::from("__inputs.x");
        assert!(!Role::Viewer.can_write(&p, None));
        assert!(!Role::Viewer.can_input());
    }

    #[test]
    fn operator_writes_inputs_only() {
        assert!(Role::Operator.can_write(&LeafPath::from("__inputs.x"), None));
        assert!(!Role::Operator.can_write(&LeafPath::from("show.x"), None));
        assert!(!Role::Operator.can_write(&LeafPath::from("__system.x"), None));
        assert!(!Role::Operator.can_write(&LeafPath::from("__test.x"), None));
    }

    #[test]
    fn service_respects_path_scope() {
        let scope = vec!["__inputs.team_a".to_string()];
        assert!(Role::Service.can_write(&LeafPath::from("__inputs.team_a.score"), Some(&scope)));
        assert!(!Role::Service.can_write(&LeafPath::from("__inputs.team_b.score"), Some(&scope)));
        // No scope: any __inputs.*
        assert!(Role::Service.can_write(&LeafPath::from("__inputs.x"), None));
    }

    #[test]
    fn test_role_writes_test_namespace() {
        assert!(Role::Test.can_write(&LeafPath::from("__test.mock"), None));
        assert!(!Role::Test.can_write(&LeafPath::from("__inputs.x"), None));
    }
}
