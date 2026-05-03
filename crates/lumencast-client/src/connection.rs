//! Single LSDP/1 WebSocket connection — open, subscribe, await
//! snapshot, then expose a frame stream.

use futures_util::{SinkExt, StreamExt};
use http::Uri;
use lumencast_protocol::frames::{ClientFrame, ServerFrame, Snapshot, Subscribe};
use lumencast_protocol::types::{SceneId, SessionId, Token};
use lumencast_protocol::{LumencastError, codec, envelope::WEBSOCKET_SUBPROTOCOL};
use thiserror::Error;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
};

/// Raised by [`Connection::open`].
#[derive(Debug, Error)]
#[allow(clippy::result_large_err)]
pub(crate) enum ConnectError {
    #[error("invalid URL: {0}")]
    Url(String),
    #[error("websocket handshake: {0}")]
    Handshake(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("encode: {0}")]
    Encode(#[from] LumencastError),
    #[error("server closed before snapshot")]
    Closed,
    #[error("server emitted an error before snapshot: {0}")]
    ServerError(String),
    #[error("expected snapshot frame as first message, got {0:?}")]
    UnexpectedFrame(ServerFrame),
}

/// Open WebSocket + completed `subscribe` handshake. Holds the live
/// stream after `snapshot`.
pub(crate) struct Connection {
    pub(crate) socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    pub(crate) snapshot: Snapshot,
}

impl Connection {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn open(
        url: &str,
        token: &Token,
        scene: Option<&SceneId>,
        session: Option<&SessionId>,
        connector: Option<Connector>,
    ) -> Result<Self, ConnectError> {
        let request = build_request(url)?;
        let (mut socket, _resp) =
            connect_async_tls_with_config(request, None, false, connector).await?;

        // Send subscribe.
        let subscribe = ClientFrame::Subscribe(Subscribe {
            token: token.clone(),
            scene: scene.cloned(),
            session: session.cloned(),
            since_sequence: None,
        });
        let s = codec::encode_client_str(&subscribe)?;
        socket.send(Message::Text(s)).await?;

        // Wait for the first frame — must be `snapshot` or `error`.
        loop {
            let Some(msg) = socket.next().await else {
                return Err(ConnectError::Closed);
            };
            let msg = msg?;
            match msg {
                Message::Text(t) => {
                    let frame = codec::decode_server_str(t.as_ref())?;
                    match frame {
                        ServerFrame::Snapshot(s) => {
                            return Ok(Self {
                                socket,
                                snapshot: s,
                            });
                        }
                        ServerFrame::Error(e) => {
                            return Err(ConnectError::ServerError(format!(
                                "{}: {}",
                                e.code, e.message
                            )));
                        }
                        other => return Err(ConnectError::UnexpectedFrame(other)),
                    }
                }
                Message::Ping(p) => {
                    socket.send(Message::Pong(p)).await?;
                }
                Message::Pong(_) => {}
                Message::Close(_) => return Err(ConnectError::Closed),
                Message::Binary(_) | Message::Frame(_) => {
                    return Err(ConnectError::Handshake(
                        tokio_tungstenite::tungstenite::Error::Protocol(
                            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                        ),
                    ));
                }
            }
        }
    }
}

#[allow(clippy::result_large_err)]
fn build_request(url: &str) -> Result<Request<()>, ConnectError> {
    let uri: Uri = url
        .parse()
        .map_err(|e: http::uri::InvalidUri| ConnectError::Url(e.to_string()))?;
    let host = uri
        .host()
        .ok_or_else(|| ConnectError::Url("missing host".into()))?
        .to_string();
    let host_header = match uri.port_u16() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    Request::builder()
        .method("GET")
        .uri(url)
        .header("Host", host_header)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Protocol", WEBSOCKET_SUBPROTOCOL)
        .body(())
        .map_err(|e| ConnectError::Url(e.to_string()))
}
