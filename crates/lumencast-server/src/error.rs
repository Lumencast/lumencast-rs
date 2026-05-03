//! Server-level error type.

use thiserror::Error;

use lumencast_protocol::LumencastError;

/// Failure raised by the server crate.
#[derive(Debug, Error)]
pub enum ServerError {
    /// IO failure (binding socket, accepting, read/write WebSocket).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Protocol-level error from `lumencast-protocol`.
    #[error("protocol: {0}")]
    Protocol(#[from] LumencastError),

    /// LSML bundle is invalid or incompatible.
    #[error("bundle: {0}")]
    Bundle(#[from] lumencast_protocol::BundleError),

    /// A scene id was used that the server does not know.
    #[error("unknown scene: {0}")]
    UnknownScene(String),

    /// A duplicate scene id was registered.
    #[error("scene already exists: {0}")]
    DuplicateScene(String),

    /// A leaf value was rejected (typically a JSON object at the top
    /// level).
    #[error("invalid value: {0}")]
    InvalidValue(String),

    /// Builder is missing a required field.
    #[error("server builder is missing: {0}")]
    BuilderMissing(&'static str),
}
