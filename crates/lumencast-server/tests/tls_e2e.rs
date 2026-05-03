//! End-to-end TLS test: spawn a Lumencast server with a self-signed
//! cert, complete a `wss://` WebSocket handshake, and verify the
//! `subscribe → snapshot` exchange.
//!
//! Requires the `tls` feature.

#![cfg(feature = "tls")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lumencast_protocol::codec;
use lumencast_protocol::frames::{ClientFrame, ServerFrame, Subscribe};
use lumencast_protocol::types::Token;
use lumencast_server::tls::TlsConfig;
use lumencast_server::{MapAuthenticator, Role, Server};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

/// `ServerCertVerifier` that accepts everything. Test-only.
#[derive(Debug)]
struct AcceptAllVerifier;

impl ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

fn install_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[tokio::test]
async fn tls_subscribe_yields_snapshot() {
    install_provider();

    // Self-signed cert + key valid for `localhost`.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("rcgen");
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();

    let mut auth = MapAuthenticator::new();
    auth.insert("op", Role::Operator);

    let srv = Server::builder()
        .listen("127.0.0.1:0")
        .auth(auth)
        .build()
        .await
        .expect("build server");
    let addr = srv.local_addr().expect("local_addr");
    let scene = srv.new_scene("main").unwrap();
    scene
        .set("show.title", serde_json::json!("Hello over TLS"))
        .unwrap();

    let tls = TlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes())
        .await
        .expect("tls config");

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        // No graceful shutdown for axum_server in v0.1; drop the
        // listener via process exit. We bound the test with stop_rx.
        tokio::select! {
            res = srv.run_tls(tls) => {
                if let Err(e) = res { tracing::error!(?e, "tls run failed"); }
            }
            _ = stop_rx => {}
        }
    });

    // Build the client TLS config that accepts our self-signed cert.
    let client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAllVerifier))
        .with_no_client_auth();

    let mut ws = connect_wss(addr, client_config).await;

    // Subscribe handshake.
    let subscribe = ClientFrame::Subscribe(Subscribe {
        token: Token::from("op"),
        scene: None,
        session: None,
        since_sequence: None,
    });
    let s = codec::encode_client_str(&subscribe).unwrap();
    ws.send(Message::Text(s)).await.expect("send subscribe");

    // Expect a snapshot.
    let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("snapshot within 3s")
        .expect("stream closed")
        .expect("ws error");
    let text = match msg {
        Message::Text(t) => t,
        other => panic!("expected text, got {other:?}"),
    };
    let frame = codec::decode_server_str(&text).expect("decode");
    match frame {
        ServerFrame::Snapshot(s) => {
            assert_eq!(s.scene_id.as_str(), "main");
            assert_eq!(
                s.state.get("show.title"),
                Some(&serde_json::json!("Hello over TLS"))
            );
        }
        other => panic!("expected snapshot, got {other:?}"),
    }

    drop(ws);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

async fn connect_wss(
    addr: SocketAddr,
    client_config: ClientConfig,
) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
    let uri = format!("wss://localhost:{}/ws", addr.port());
    let mut req: Request<()> = uri.into_client_request().expect("uri");
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", "lsdp.v1".parse().unwrap());
    req.headers_mut()
        .insert("Sec-WebSocket-Key", generate_key().parse().unwrap());

    // Connect to the actual address (DNS for "localhost" might resolve
    // to ::1 or 127.0.0.1; the TLS SNI uses "localhost" via the URI).
    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("tcp connect");

    let (ws, _resp) = tokio_tungstenite::client_async_tls_with_config(
        req,
        stream,
        None,
        Some(Connector::Rustls(Arc::new(client_config))),
    )
    .await
    .expect("wss handshake");
    ws
}
