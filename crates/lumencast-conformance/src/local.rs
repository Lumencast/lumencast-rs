//! Fixtures defined inline. They mirror the shape that will arrive
//! upstream and are stable: every implementation MUST round-trip
//! identical wire bytes.

use lumencast_protocol::codec;
use lumencast_protocol::frames::{
    ClientFrame, Delta, ErrorFrame, Input, Ping, Pong, SceneChanged, ServerFrame, Snapshot,
    Subscribe,
};
use lumencast_protocol::types::{Patch, SceneId, SceneVersion, SessionId, Token};
use lumencast_protocol::{ErrorCode, LeafPath};
use serde_json::json;

/// Canonical placeholder bundle hash used in fixtures.
pub fn placeholder_version() -> SceneVersion {
    SceneVersion::from("sha256:0000000000000000000000000000000000000000000000000000000000000000")
}

/// All baseline server frames.
pub fn server_fixtures() -> Vec<ServerFrame> {
    vec![
        ServerFrame::Snapshot(Snapshot {
            seq: 1,
            scene_id: SceneId::from("test-scene"),
            scene_version: placeholder_version(),
            state: [
                ("title".to_string(), json!("Hello")),
                ("count".to_string(), json!(0)),
            ]
            .into_iter()
            .collect(),
            ts: None,
        }),
        ServerFrame::Delta(Delta {
            seq: 2,
            patches: vec![Patch::new(LeafPath::from("count"), json!(1))],
            ts: None,
            cause: None,
        }),
        ServerFrame::SceneChanged(SceneChanged {
            seq: 3,
            scene_id: SceneId::from("next"),
            scene_version: placeholder_version(),
            ts: None,
            from_scene_id: None,
            transition: None,
        }),
        ServerFrame::Error(ErrorFrame {
            seq: 4,
            code: ErrorCode::WriteForbidden,
            message: "viewer cannot write".into(),
            recoverable: true,
            retry_after_ms: None,
            path: None,
            ts: None,
        }),
        ServerFrame::Pong(Pong::default()),
    ]
}

/// All baseline client frames.
pub fn client_fixtures() -> Vec<ClientFrame> {
    vec![
        ClientFrame::Subscribe(Subscribe {
            token: Token::from("op"),
            scene: None,
            session: None,
            since_sequence: None,
        }),
        ClientFrame::Subscribe(Subscribe {
            token: Token::from("test"),
            scene: Some(SceneId::from("preview")),
            session: Some(SessionId::from("sess-1")),
            since_sequence: None,
        }),
        ClientFrame::Input(Input {
            patches: vec![Patch::new(LeafPath::from("__inputs.title"), json!("New"))],
            client_msg_id: None,
        }),
        ClientFrame::Ping(Ping::default()),
    ]
}

/// Returns `true` if every fixture round-trips through the codec
/// without loss. This MUST hold for any LSDP/1-conformant
/// implementation.
pub fn round_trips_ok() -> Result<(), String> {
    for f in server_fixtures() {
        let bytes = codec::encode_server(&f).map_err(|e| format!("encode_server: {e}"))?;
        let parsed = codec::decode_server(&bytes).map_err(|e| format!("decode_server: {e}"))?;
        if parsed != f {
            return Err(format!("server round-trip mismatch on {}", f.type_str()));
        }
    }
    for f in client_fixtures() {
        let bytes = codec::encode_client(&f).map_err(|e| format!("encode_client: {e}"))?;
        let parsed = codec::decode_client(&bytes).map_err(|e| format!("decode_client: {e}"))?;
        if parsed != f {
            return Err(format!("client round-trip mismatch on {}", f.type_str()));
        }
    }
    Ok(())
}
