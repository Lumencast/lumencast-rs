//! TLS support — gated behind the `tls` feature.
//!
//! This module exposes [`TlsConfig`], a thin wrapper over
//! [`axum_server::tls_rustls::RustlsConfig`], plus an extension method
//! on [`Server`] to terminate TLS in-process.
//!
//! For most production deployments, prefer terminating TLS at a
//! reverse proxy (nginx, Caddy, AWS ALB). The in-process option is
//! useful for self-contained binaries and embedded contexts.
//!
//! # Crypto provider
//!
//! `axum-server` is depended upon with the `tls-rustls-no-provider`
//! feature, which means **the calling binary MUST install a rustls
//! crypto provider before [`Server::run_tls`] is invoked**. Typical:
//!
//! ```ignore
//! rustls::crypto::aws_lc_rs::default_provider()
//!     .install_default()
//!     .expect("install rustls provider");
//! ```
//!
//! or `rustls::crypto::ring::default_provider()`.

use std::path::Path;

pub use axum_server::tls_rustls::RustlsConfig;

use crate::error::ServerError;
use crate::server::{Server, build_router};

/// Server-side TLS configuration.
///
/// Build via [`TlsConfig::from_pem_file`] (paths) or
/// [`TlsConfig::from_pem`] (bytes).
#[derive(Clone)]
pub struct TlsConfig(pub(crate) RustlsConfig);

impl TlsConfig {
    /// Load a PEM-encoded certificate chain and private key from disk.
    pub async fn from_pem_file(
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<Self, ServerError> {
        let inner = RustlsConfig::from_pem_file(cert.as_ref(), key.as_ref())
            .await
            .map_err(ServerError::Io)?;
        Ok(Self(inner))
    }

    /// Build a config from in-memory PEM bytes.
    pub async fn from_pem(cert: Vec<u8>, key: Vec<u8>) -> Result<Self, ServerError> {
        let inner = RustlsConfig::from_pem(cert, key)
            .await
            .map_err(ServerError::Io)?;
        Ok(Self(inner))
    }
}

impl Server {
    /// Run the server with TLS termination. Consumes `self`.
    ///
    /// Reuses the [`TcpListener`](tokio::net::TcpListener) bound by
    /// [`crate::ServerBuilder::build`] — no port juggling.
    pub async fn run_tls(self, tls: TlsConfig) -> Result<(), ServerError> {
        let std_listener = self.listener.into_std()?;
        std_listener.set_nonblocking(true)?;
        axum_server::from_tcp_rustls(std_listener, tls.0)
            .serve(build_router(self.inner).into_make_service())
            .await?;
        Ok(())
    }
}
