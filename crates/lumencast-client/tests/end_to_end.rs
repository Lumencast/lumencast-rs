//! End-to-end client tests against a real `lumencast-server`.

use std::time::Duration;

use futures_util::StreamExt;
use lumencast_client::{Client, Event, Status};
use lumencast_server::{MapAuthenticator, Role, Server};
use serde_json::json;
use tokio::sync::oneshot;

async fn boot_server() -> (Server, std::net::SocketAddr) {
    let mut auth = MapAuthenticator::new();
    auth.insert("op", Role::Operator);
    auth.insert("op2", Role::Operator);
    auth.insert("viewer", Role::Viewer);

    let srv = Server::builder()
        .listen("127.0.0.1:0")
        .auth(auth)
        .build()
        .await
        .expect("build server");
    let addr = srv.local_addr().expect("addr");
    (srv, addr)
}

async fn next_event(events: &mut lumencast_client::EventStream) -> Event {
    tokio::time::timeout(Duration::from_secs(3), events.next())
        .await
        .expect("event within 3s")
        .expect("stream open")
}

async fn skip_status(events: &mut lumencast_client::EventStream) -> Event {
    loop {
        match next_event(events).await {
            Event::Status(_) => {}
            other => return other,
        }
    }
}

#[tokio::test]
async fn connect_yields_snapshot_then_delta() {
    let (srv, addr) = boot_server().await;
    let scene = srv.new_scene("main").unwrap();
    scene.set("show.title", json!("Hello")).unwrap();

    let scene_for_drive = scene.clone();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let client = Client::builder()
        .url(format!("ws://{addr}/ws"))
        .token("op")
        .build()
        .await
        .unwrap();
    let mut events = client.events();

    match skip_status(&mut events).await {
        Event::Snapshot(s) => {
            assert_eq!(s.scene_id.as_str(), "main");
            assert_eq!(s.state.get("show.title"), Some(&json!("Hello")));
        }
        other => panic!("expected snapshot, got {other:?}"),
    }

    scene_for_drive.set("show.title", json!("Updated")).unwrap();

    match skip_status(&mut events).await {
        Event::Delta(d) => {
            assert_eq!(d.patches.len(), 1);
            assert_eq!(d.patches[0].path.as_str(), "show.title");
            assert_eq!(d.patches[0].value, json!("Updated"));
        }
        other => panic!("expected delta, got {other:?}"),
    }

    client.disconnect().await.unwrap();
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn set_token_swaps_seamlessly() {
    let (srv, addr) = boot_server().await;
    let scene = srv.new_scene("main").unwrap();
    scene.set("v", json!(1)).unwrap();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let client = Client::builder()
        .url(format!("ws://{addr}/ws"))
        .token("op")
        .build()
        .await
        .unwrap();
    let mut events = client.events();

    // Drain initial snapshot.
    match skip_status(&mut events).await {
        Event::Snapshot(_) => {}
        other => panic!("expected initial snapshot, got {other:?}"),
    }

    // Rotate token.
    client.set_token("op2").await.unwrap();

    // The manager re-emits a snapshot for the new connection.
    match skip_status(&mut events).await {
        Event::Snapshot(s) => {
            assert_eq!(s.scene_id.as_str(), "main");
            assert_eq!(s.state.get("v"), Some(&json!(1)));
        }
        other => panic!("expected snapshot after rotation, got {other:?}"),
    }

    client.disconnect().await.unwrap();
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}

#[tokio::test]
async fn auth_failure_closes_with_no_retry() {
    let (srv, addr) = boot_server().await;
    let _scene = srv.new_scene("main").unwrap();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        srv.run_with_shutdown(async {
            let _ = stop_rx.await;
        })
        .await
        .unwrap();
    });

    let client = Client::builder()
        .url(format!("ws://{addr}/ws"))
        .token("garbage")
        .max_reconnect_attempts(Some(1))
        .build()
        .await
        .unwrap();
    let mut events = client.events();

    // We expect to see Connecting → Disconnected (server sends an
    // AUTH_DENIED error then closes; the client treats that as a
    // failed handshake, retries once, then exhausts attempts).
    let mut saw_disconnected = false;
    let mut iterations = 0;
    while iterations < 10 {
        iterations += 1;
        let Some(ev) = tokio::time::timeout(Duration::from_secs(3), events.next())
            .await
            .ok()
            .flatten()
        else {
            break;
        };
        if matches!(ev, Event::Status(Status::Disconnected)) {
            saw_disconnected = true;
            break;
        }
    }
    assert!(saw_disconnected, "client should have given up");

    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), server_task).await;
}
