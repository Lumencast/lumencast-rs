//! Authentication trait and a [`MapAuthenticator`] for tests/dev.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use thiserror::Error;

use crate::role::Role;

/// Identity attached to a connection after token validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Subject (user/operator id, opaque).
    pub subject: String,
    /// Role this connection acts as.
    pub role: Role,
    /// For `service` tokens, optional list of path prefixes the holder
    /// is allowed to write.
    pub paths: Option<Vec<String>>,
}

/// Failure reason returned by an [`Authenticator`].
#[derive(Debug, Clone, Error)]
pub enum AuthError {
    /// Token is unknown, expired, or revoked.
    #[error("auth denied")]
    Denied,

    /// Authenticator could not reach its backend (DB, key server, ...).
    #[error("authenticator unavailable: {0}")]
    Unavailable(String),
}

/// Token validation contract.
///
/// LSDP is token-agnostic — implementors decide how tokens are
/// formatted (JWT, opaque, mTLS-derived, ...).
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    /// Validate a token and return the resulting [`Identity`].
    async fn authenticate(&self, token: &str) -> Result<Identity, AuthError>;
}

/// In-memory authenticator backed by a token → identity map.
///
/// Intended for tests, examples, and local development. Not for prod.
#[derive(Debug, Default, Clone)]
pub struct MapAuthenticator {
    inner: Arc<RwLock<HashMap<String, Identity>>>,
}

impl MapAuthenticator {
    /// Build an empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a token mapping to a role with no scope claim.
    pub fn insert(&mut self, token: impl Into<String>, role: Role) -> &mut Self {
        let token = token.into();
        let identity = Identity {
            subject: token.clone(),
            role,
            paths: None,
        };
        self.inner.write().insert(token, identity);
        self
    }

    /// Register a token mapping to a full identity (used for service
    /// tokens that need a `paths` scope claim).
    pub fn insert_identity(&mut self, token: impl Into<String>, identity: Identity) -> &mut Self {
        self.inner.write().insert(token.into(), identity);
        self
    }

    /// Drop a token from the map.
    pub fn remove(&mut self, token: &str) -> Option<Identity> {
        self.inner.write().remove(token)
    }

    /// Drop every registered token. Used by interop test resets.
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// Number of registered tokens.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Returns `true` if no tokens are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }
}

#[async_trait]
impl Authenticator for MapAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<Identity, AuthError> {
        self.inner
            .read()
            .get(token)
            .cloned()
            .ok_or(AuthError::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn map_auth_round_trip() {
        let mut a = MapAuthenticator::new();
        a.insert("op", Role::Operator);
        let id = a.authenticate("op").await.unwrap();
        assert_eq!(id.role, Role::Operator);
        assert!(matches!(
            a.authenticate("nope").await.unwrap_err(),
            AuthError::Denied
        ));
    }
}
