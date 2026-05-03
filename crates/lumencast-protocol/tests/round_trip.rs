//! Cross-module round-trip tests for the protocol crate.

use lumencast_protocol::codec;
use lumencast_protocol::frames::{
    ClientFrame, Delta, ErrorFrame, Input, SceneChanged, ServerFrame, Snapshot, Subscribe,
};
use lumencast_protocol::types::{Patch, SceneId, SceneVersion, SessionId, Token};
use lumencast_protocol::{ErrorCode, LeafPath};
use serde_json::json;

fn version() -> SceneVersion {
    SceneVersion::from("sha256:0000000000000000000000000000000000000000000000000000000000000000")
}

fn round_trip_server(frame: ServerFrame) {
    let bytes = codec::encode_server(&frame).unwrap();
    let parsed = codec::decode_server(&bytes).unwrap();
    assert_eq!(frame, parsed, "server round-trip mismatch");
}

fn round_trip_client(frame: ClientFrame) {
    let bytes = codec::encode_client(&frame).unwrap();
    let parsed = codec::decode_client(&bytes).unwrap();
    assert_eq!(frame, parsed, "client round-trip mismatch");
}

#[test]
fn server_snapshot() {
    round_trip_server(ServerFrame::Snapshot(Snapshot {
        seq: 1,
        scene_id: SceneId::from("main-stage"),
        scene_version: version(),
        state: [
            ("show.title".to_string(), json!("Live Now")),
            ("score.home".to_string(), json!(0)),
            ("players".to_string(), json!([])),
        ]
        .into_iter()
        .collect(),
        ts: Some("2026-05-03T12:00:00Z".into()),
    }));
}

#[test]
fn server_delta() {
    round_trip_server(ServerFrame::Delta(Delta {
        seq: 42,
        patches: vec![
            Patch::new(LeafPath::from("score.home"), json!(7)),
            Patch::new(LeafPath::from("show.title"), json!("Match Point")),
        ],
        ts: None,
    }));
}

#[test]
fn server_scene_changed() {
    round_trip_server(ServerFrame::SceneChanged(SceneChanged {
        seq: 100,
        scene_id: SceneId::from("intermission"),
        scene_version: version(),
        ts: None,
    }));
}

#[test]
fn server_error() {
    round_trip_server(ServerFrame::Error(ErrorFrame {
        seq: 50,
        code: ErrorCode::WriteForbidden,
        message: "viewer cannot send input".into(),
        recoverable: true,
        retry_after_ms: None,
        ts: None,
    }));
}

#[test]
fn server_pong() {
    round_trip_server(ServerFrame::Pong);
}

#[test]
fn client_subscribe_minimal() {
    round_trip_client(ClientFrame::Subscribe(Subscribe {
        token: Token::from("op-token"),
        scene: None,
        session: None,
    }));
}

#[test]
fn client_subscribe_test_mode() {
    round_trip_client(ClientFrame::Subscribe(Subscribe {
        token: Token::from("test-token"),
        scene: Some(SceneId::from("preview")),
        session: Some(SessionId::from("sess-123")),
    }));
}

#[test]
fn client_input() {
    round_trip_client(ClientFrame::Input(Input {
        patches: vec![
            Patch::new(LeafPath::from("__inputs.show_title"), json!("New title")),
            Patch::new(LeafPath::from("__inputs.show_visible"), json!(true)),
        ],
    }));
}

#[test]
fn client_ping() {
    round_trip_client(ClientFrame::Ping);
}

#[test]
fn error_code_wire_string() {
    let bytes = codec::encode_server(&ServerFrame::Error(ErrorFrame {
        seq: 1,
        code: ErrorCode::AuthDenied,
        message: "x".into(),
        recoverable: false,
        retry_after_ms: None,
        ts: None,
    }))
    .unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(
        s.contains("\"code\":\"AUTH_DENIED\""),
        "wire form mismatch: {s}"
    );
}

#[test]
fn envelope_v_field_present() {
    let bytes = codec::encode_server(&ServerFrame::Pong).unwrap();
    let s = std::str::from_utf8(&bytes).unwrap();
    assert!(s.contains("\"v\":1"), "missing v=1 envelope: {s}");
    assert!(s.contains("\"type\":\"pong\""), "missing type: {s}");
}

#[test]
fn rejects_object_value_in_input() {
    let raw = br#"{"v":1,"type":"input","patches":[{"path":"__inputs.x","value":{"a":1}}]}"#;
    let err = codec::decode_client(raw).unwrap_err();
    assert!(matches!(
        err,
        lumencast_protocol::LumencastError::InvalidValue { .. }
    ));
}
