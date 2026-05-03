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
        cause: None,
    }));
}

#[test]
fn server_scene_changed() {
    round_trip_server(ServerFrame::SceneChanged(SceneChanged {
        seq: 100,
        scene_id: SceneId::from("intermission"),
        scene_version: version(),
        ts: None,
        from_scene_id: None,
        transition: None,
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
        path: None,
        ts: None,
    }));
}

#[test]
fn server_pong() {
    round_trip_server(ServerFrame::Pong(
        lumencast_protocol::frames::Pong::default(),
    ));
}

#[test]
fn client_subscribe_minimal() {
    round_trip_client(ClientFrame::Subscribe(Subscribe {
        token: Token::from("op-token"),
        scene: None,
        session: None,
        since_sequence: None,
    }));
}

#[test]
fn client_subscribe_test_mode() {
    round_trip_client(ClientFrame::Subscribe(Subscribe {
        token: Token::from("test-token"),
        scene: Some(SceneId::from("preview")),
        session: Some(SessionId::from("sess-123")),
        since_sequence: None,
    }));
}

#[test]
fn client_input() {
    round_trip_client(ClientFrame::Input(Input {
        patches: vec![
            Patch::new(LeafPath::from("__inputs.show_title"), json!("New title")),
            Patch::new(LeafPath::from("__inputs.show_visible"), json!(true)),
        ],
        client_msg_id: None,
    }));
}

#[test]
fn client_ping() {
    round_trip_client(ClientFrame::Ping(
        lumencast_protocol::frames::Ping::default(),
    ));
}

#[test]
fn error_code_wire_string() {
    let bytes = codec::encode_server(&ServerFrame::Error(ErrorFrame {
        seq: 1,
        code: ErrorCode::AuthDenied,
        message: "x".into(),
        recoverable: false,
        retry_after_ms: None,
        path: None,
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
    let bytes = codec::encode_server(&ServerFrame::Pong(
        lumencast_protocol::frames::Pong::default(),
    ))
    .unwrap();
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

// ─────────────────────────────────────────────────────────────────────
// LSDP/1.1 — additive frame surface round-trip tests.
// Mirrors lumencast-go/protocol/protocol_test.go and
// lumencast-js/packages/protocol/tests/v1_1.test.ts.
// Note: serde_json encodes keys alphabetically, so we assert on the
// decoded value rather than byte-equality of the encoded string.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn subprotocol_constants() {
    use lumencast_protocol::envelope::{
        WEBSOCKET_SUBPROTOCOL, WEBSOCKET_SUBPROTOCOL_V1_1, WEBSOCKET_SUBPROTOCOLS,
    };
    assert_eq!(WEBSOCKET_SUBPROTOCOL, "lsdp.v1");
    assert_eq!(WEBSOCKET_SUBPROTOCOL_V1_1, "lsdp.v1.1");
    assert_eq!(WEBSOCKET_SUBPROTOCOLS, &["lsdp.v1.1", "lsdp.v1"]);
}

#[test]
fn subscribe_with_since_sequence_round_trips() {
    let frame = ClientFrame::Subscribe(Subscribe {
        token: Token::from("t"),
        scene: None,
        session: None,
        since_sequence: Some(12345),
    });
    round_trip_client(frame);
}

#[test]
fn bare_subscribe_omits_since_sequence_on_wire() {
    // 1.0-shape regression: when since_sequence is None, the field MUST
    // NOT appear in the encoded JSON.
    let frame = ClientFrame::Subscribe(Subscribe {
        token: Token::from("t"),
        scene: None,
        session: None,
        since_sequence: None,
    });
    let bytes = codec::encode_client(&frame).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(
        !raw.contains("since_sequence"),
        "leaked since_sequence: {raw}"
    );
}

#[test]
fn input_with_client_msg_id_round_trips() {
    let frame = ClientFrame::Input(Input {
        patches: vec![Patch::new(LeafPath::from("__inputs.title"), json!("Hello"))],
        client_msg_id: Some("ui-9f3a".to_string()),
    });
    let bytes = codec::encode_client(&frame).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(raw.contains(r#""client_msg_id":"ui-9f3a""#));
    round_trip_client(frame);
}

#[test]
#[allow(clippy::similar_names)] // ping/pong are domain-correct here
fn ping_pong_nonce_round_trips() {
    use lumencast_protocol::frames::{Ping, Pong};

    round_trip_client(ClientFrame::Ping(Ping {
        nonce: Some("probe-7a2c".to_string()),
    }));
    round_trip_server(ServerFrame::Pong(Pong {
        nonce: Some("probe-7a2c".to_string()),
    }));

    // Bare ping/pong omit the nonce field on the wire.
    let raw_ping = codec::encode_client(&ClientFrame::Ping(Ping::default())).unwrap();
    let raw_ping = std::str::from_utf8(&raw_ping).unwrap();
    assert!(
        !raw_ping.contains("nonce"),
        "bare ping leaked nonce: {raw_ping}"
    );

    let raw_pong = codec::encode_server(&ServerFrame::Pong(Pong::default())).unwrap();
    let raw_pong = std::str::from_utf8(&raw_pong).unwrap();
    assert!(
        !raw_pong.contains("nonce"),
        "bare pong leaked nonce: {raw_pong}"
    );
}

#[test]
fn unsubscribe_round_trips() {
    use lumencast_protocol::frames::Unsubscribe;
    let frame = ClientFrame::Unsubscribe(Unsubscribe::default());
    round_trip_client(frame);
}

#[test]
fn delta_with_cause_and_transition_round_trips() {
    use lumencast_protocol::types::{Cause, Easing, TransitionSpec};

    let frame = ServerFrame::Delta(Delta {
        seq: 7,
        patches: vec![Patch::with_transition(
            LeafPath::from("score"),
            json!(42),
            TransitionSpec::Tween {
                duration_ms: Some(500),
                easing: Some(Easing::EaseOut),
            },
        )],
        ts: None,
        cause: Some(Cause {
            source: "operator:alice".to_string(),
            input_id: Some("ui-9f3a".to_string()),
        }),
    });
    let bytes = codec::encode_server(&frame).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(raw.contains(r#""kind":"tween""#));
    assert!(raw.contains(r#""easing":"ease-out""#));
    assert!(raw.contains(r#""source":"operator:alice""#));
    round_trip_server(frame);
}

#[test]
fn scene_changed_with_transition_round_trips() {
    use lumencast_protocol::types::SceneTransition;

    let frame = ServerFrame::SceneChanged(SceneChanged {
        seq: 100,
        scene_id: SceneId::from("scene-b"),
        scene_version: SceneVersion::from(
            "sha256:b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0",
        ),
        ts: None,
        from_scene_id: Some(SceneId::from("scene-a")),
        transition: Some(SceneTransition {
            kind: "crossfade".to_string(),
            duration_ms: Some(600),
        }),
    });
    let bytes = codec::encode_server(&frame).unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(raw.contains(r#""from_scene_id":"scene-a""#));
    assert!(raw.contains(r#""kind":"crossfade""#));
    round_trip_server(frame);
}

#[test]
fn forward_compat_1_0_decodes_1_1_delta() {
    let raw = r#"{"v":1,"type":"delta","seq":1,"patches":[{"path":"x","value":1}],"cause":{"source":"adapter:http_poll"}}"#;
    let frame = codec::decode_server(raw.as_bytes()).unwrap();
    if let ServerFrame::Delta(d) = frame {
        assert_eq!(d.seq, 1);
        assert_eq!(
            d.cause.as_ref().map(|c| c.source.as_str()),
            Some("adapter:http_poll")
        );
    } else {
        panic!("not a delta");
    }
}
