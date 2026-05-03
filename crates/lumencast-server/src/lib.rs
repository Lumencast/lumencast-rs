//! Server kit for Lumencast on `axum` + `tokio`.
//!
//! See [`Server`] for the entry point.
//!
//! ```no_run
//! use lumencast_server::{MapAuthenticator, Role, Server};
//! use serde_json::json;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut auth = MapAuthenticator::new();
//! auth.insert("op-token", Role::Operator);
//!
//! let srv = Server::builder()
//!     .listen("127.0.0.1:0")
//!     .auth(auth)
//!     .build()
//!     .await?;
//!
//! let scene = srv.new_scene("main-stage")?;
//! scene.set("show.title", json!("Hello"))?;
//!
//! srv.run().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Token rotation
//!
//! LSDP/1 §8 specifies token rotation as a **runtime-side** concern:
//! the runtime opens a fresh WebSocket with the new token, waits for
//! the new `snapshot`, and closes the old WebSocket. The server
//! requires no special handling — every connection is independent and
//! authenticates from scratch. Just configure your
//! [`Authenticator`] to accept rotated credentials.
//!
//! # TLS
//!
//! The `tls` feature gates an in-process TLS path via `axum-server` +
//! `rustls`. See the `tls` module (only present when the feature is
//! enabled). For most production deployments, prefer terminating TLS
//! at a reverse proxy.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod adapters;
mod auth;
mod bundle_route;
mod config;
mod error;
mod role;
mod scene;
mod server;
mod store;
#[cfg(feature = "interop-control-plane")]
pub mod test_control;
#[cfg(feature = "tls")]
pub mod tls;
mod ws_handler;

pub use auth::{AuthError, Authenticator, Identity, MapAuthenticator};
pub use config::ServerConfig;
pub use error::ServerError;
pub use role::Role;
pub use scene::Scene;
pub use server::{Server, ServerBuilder, ServerHandle};

#[cfg(feature = "tls")]
pub use tls::{RustlsConfig, TlsConfig};

// Re-exports from the protocol crate so server users do not need to
// pull it in directly for the most common types.
pub use lumencast_protocol::{
    Bundle, BundleError, ErrorCode, LeafPath, LumencastError, OperatorInput, SceneId, SceneVersion,
    SessionId, Token,
};
