//! End-to-end tests against a real bound server: subscribe handshake,
//! delta fan-out, role enforcement, scene swap.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lumencast_protocol::codec;
use lumencast_protocol::frames::{ClientFrame, Input, ServerFrame, Subscribe};
use lumencast_protocol::types::{Patch, Token};
use lumencast_protocol::{ErrorCode, LeafPath};
use lumencast_server::{MapAuthenticator, Role, Server};
use serde_json::json;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::{Request, Uri as TungUri};
use tokio_tungstenite::tungstenite::protocol::Message;

async fn start_server() -> (Server, SocketAddr, MapAuthenticator) {
    let mut auth = MapAuthenticator::new();
    auth.insert("op", Role::Operator);
    auth.insert("viewer", Role::Viewer);

    let srv = Server::builder()
        .listen("127.0.0.1:0")
        .auth(auth.clone())
        .build()
        .await
        .expect("server builds");
    let addr = srv.local_addr().expect("local_addr");
    (srv, addr, auth)
}

fn build_request(addr: SocketAddr) -> Request<()> {
    let uri: TungUri = format!("ws://{addr}/ws").parse().expect("uri");
    let host = uri.host().unwrap().to_string();
    let host_header = match uri.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("Host", host_header)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Protocol", "lsdp.v1")
        .body(())
        .expect("request")
}

async fn connect(
    addr: SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let req = build_request(addr);
    let (stream, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .expect("connect");
    stream
}

async fn send_client(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    frame: &ClientFrame,
) {
    let s = codec::encode_client_str(frame).expect("encode");
    ws.send(Message::Text(s)).await.expect("send");
}

async fn recv_server(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> ServerFrame {
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("stream closed")
        .expect("ws error");
    match msg {
        Message::Text(t) => codec::decode_server_str(&t).expect("decode"),
        other => panic!("unexpected ws message: {other:?}"),
    }
}

#[tokio::test]
async fn subscribe_yields_snapshot() {
    let (srv, addr, _auth) = start_server().await;
    let scene = srv.new_scene("main").unwrap();
    scene.set("show.title", json!("Hello")).unwrap();

    let (tx, rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });

    let mut ws = connect(addr).await;
    send_client(
        &mut ws,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("op"),
            scene: None,
            session: None,
        }),
    )
    .await;

    let frame = recv_server(&mut ws).await;
    match frame {
        ServerFrame::Snapshot(s) => {
            assert_eq!(s.seq, 1);
            assert_eq!(s.scene_id.as_str(), "main");
            assert_eq!(s.state.get("show.title"), Some(&json!("Hello")));
        }
        other => panic!("expected snapshot, got {other:?}"),
    }

    drop(ws);
    let _ = tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn operator_input_fans_out_as_delta() {
    let (srv, addr, _auth) = start_server().await;
    let _scene = srv.new_scene("main").unwrap();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    // Two clients: one operator, one viewer (just observes).
    let mut op = connect(addr).await;
    let mut viewer = connect(addr).await;

    send_client(
        &mut op,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("op"),
            scene: None,
            session: None,
        }),
    )
    .await;
    send_client(
        &mut viewer,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("viewer"),
            scene: None,
            session: None,
        }),
    )
    .await;

    // Drain snapshots.
    let _ = recv_server(&mut op).await;
    let _ = recv_server(&mut viewer).await;

    // Operator sends an input.
    send_client(
        &mut op,
        &ClientFrame::Input(Input {
            patches: vec![Patch::new(
                LeafPath::from("__inputs.title"),
                json!("Hello world"),
            )],
        }),
    )
    .await;

    // Both see the resulting delta.
    for ws in [&mut op, &mut viewer] {
        match recv_server(ws).await {
            ServerFrame::Delta(d) => {
                assert_eq!(d.seq, 2);
                assert_eq!(d.patches.len(), 1);
                assert_eq!(d.patches[0].path.as_str(), "__inputs.title");
                assert_eq!(d.patches[0].value, json!("Hello world"));
            }
            other => panic!("expected delta, got {other:?}"),
        }
    }

    drop(op);
    drop(viewer);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn viewer_input_is_rejected() {
    let (srv, addr, _auth) = start_server().await;
    let _scene = srv.new_scene("main").unwrap();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let mut ws = connect(addr).await;
    send_client(
        &mut ws,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("viewer"),
            scene: None,
            session: None,
        }),
    )
    .await;
    let _ = recv_server(&mut ws).await; // snapshot

    send_client(
        &mut ws,
        &ClientFrame::Input(Input {
            patches: vec![Patch::new(LeafPath::from("__inputs.title"), json!("nope"))],
        }),
    )
    .await;

    match recv_server(&mut ws).await {
        ServerFrame::Error(e) => {
            assert_eq!(e.code, ErrorCode::WriteForbidden);
            assert!(e.recoverable, "WRITE_FORBIDDEN is recoverable");
        }
        other => panic!("expected error frame, got {other:?}"),
    }

    drop(ws);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn auth_denied_closes() {
    let (srv, addr, _auth) = start_server().await;
    let _scene = srv.new_scene("main").unwrap();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let mut ws = connect(addr).await;
    send_client(
        &mut ws,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("garbage"),
            scene: None,
            session: None,
        }),
    )
    .await;

    match recv_server(&mut ws).await {
        ServerFrame::Error(e) => {
            assert_eq!(e.code, ErrorCode::AuthDenied);
            assert!(!e.recoverable);
        }
        other => panic!("expected error frame, got {other:?}"),
    }

    drop(ws);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

async fn http_get(addr: SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::with_capacity(2048);
    stream.read_to_end(&mut buf).await.expect("read");
    String::from_utf8_lossy(&buf).to_string()
}

#[tokio::test]
async fn register_bundle_seeds_state_and_serves_bytes() {
    use lumencast_protocol::Bundle;

    let (srv, addr, _auth) = start_server().await;
    let bundle_json = serde_json::to_string(&json!({
        "lsml": "1.0",
        "scene_id": "scoreboard",
        "scene_version": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "layout": { "kind": "frame", "size": { "w": 1920, "h": 1080 }, "children": [] },
        "defaults": { "score.home": 0, "score.away": 0, "show.title": "Live" }
    }))
    .unwrap();
    let bundle = Bundle::parse_str(&bundle_json).unwrap();
    let scene = srv.register_bundle(bundle).unwrap();

    let version = scene.version().clone();
    assert!(version.is_well_formed());
    let hex = version
        .as_str()
        .strip_prefix("sha256:")
        .unwrap()
        .to_string();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    // 1. WS subscribe sees the seeded defaults in the snapshot.
    let mut ws = connect(addr).await;
    send_client(
        &mut ws,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("op"),
            scene: None,
            session: None,
        }),
    )
    .await;
    match recv_server(&mut ws).await {
        ServerFrame::Snapshot(s) => {
            assert_eq!(s.scene_version.as_str(), version.as_str());
            assert_eq!(s.state.get("score.home"), Some(&json!(0)));
            assert_eq!(s.state.get("show.title"), Some(&json!("Live")));
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
    drop(ws);

    // 2. HTTP fetch of the bundle at the content-addressed URL succeeds.
    let response = http_get(addr, &format!("/scenes/scoreboard/{hex}")).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("application/json"));
    assert!(response.contains("immutable"));
    assert!(response.contains("\"scene_id\":\"scoreboard\""));

    // 3. Wrong version → 404.
    let bad = http_get(addr, &format!("/scenes/scoreboard/{}", "0".repeat(64))).await;
    assert!(bad.starts_with("HTTP/1.1 404"), "{bad}");

    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn scene_swap_emits_scene_changed_then_snapshot() {
    let (srv, addr, _auth) = start_server().await;
    let _a = srv.new_scene("a").unwrap();
    let b = srv.new_scene("b").unwrap();
    b.set("hello", json!("from-b")).unwrap();

    let handle = srv.handle();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let mut ws = connect(addr).await;
    send_client(
        &mut ws,
        &ClientFrame::Subscribe(Subscribe {
            token: Token::from("op"),
            scene: None,
            session: None,
        }),
    )
    .await;
    match recv_server(&mut ws).await {
        ServerFrame::Snapshot(s) => assert_eq!(s.scene_id.as_str(), "a"),
        f => panic!("expected snapshot of `a`, got {f:?}"),
    }

    // Trigger the swap server-side.
    handle
        .set_active_scene(lumencast_protocol::SceneId::from("b"))
        .unwrap();

    match recv_server(&mut ws).await {
        ServerFrame::SceneChanged(c) => assert_eq!(c.scene_id.as_str(), "b"),
        f => panic!("expected scene_changed, got {f:?}"),
    }
    match recv_server(&mut ws).await {
        ServerFrame::Snapshot(s) => {
            assert_eq!(s.seq, 1, "snapshot after scene_changed must reset seq to 1");
            assert_eq!(s.scene_id.as_str(), "b");
            assert_eq!(s.state.get("hello"), Some(&json!("from-b")));
        }
        f => panic!("expected snapshot of `b`, got {f:?}"),
    }

    drop(ws);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}
