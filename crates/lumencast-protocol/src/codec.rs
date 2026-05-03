//! JSON encode/decode for whole frames.
//!
//! These routines wrap [`crate::frames::ServerFrame`] and
//! [`crate::frames::ClientFrame`] with the LSDP `v: 1` envelope and
//! validate it on the way back in.

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::envelope::PROTOCOL_VERSION;
use crate::errors::LumencastError;
use crate::frames::{ClientFrame, ServerFrame};

/// Encode a server frame to a JSON byte vector with the `v: 1` envelope.
pub fn encode_server(frame: &ServerFrame) -> Result<Vec<u8>, LumencastError> {
    encode_with_envelope(frame)
}

/// Decode a server frame from JSON bytes, validating the envelope.
pub fn decode_server(input: &[u8]) -> Result<ServerFrame, LumencastError> {
    decode_with_envelope::<ServerFrame>(input).and_then(validate_server)
}

/// Encode a client frame to a JSON byte vector with the `v: 1` envelope.
pub fn encode_client(frame: &ClientFrame) -> Result<Vec<u8>, LumencastError> {
    encode_with_envelope(frame)
}

/// Decode a client frame from JSON bytes, validating the envelope.
pub fn decode_client(input: &[u8]) -> Result<ClientFrame, LumencastError> {
    decode_with_envelope::<ClientFrame>(input).and_then(validate_client)
}

/// Encode a server frame to a UTF-8 JSON string.
pub fn encode_server_str(frame: &ServerFrame) -> Result<String, LumencastError> {
    encode_str_with_envelope(frame)
}

/// Decode a server frame from a UTF-8 JSON string.
pub fn decode_server_str(input: &str) -> Result<ServerFrame, LumencastError> {
    decode_server(input.as_bytes())
}

/// Encode a client frame to a UTF-8 JSON string.
pub fn encode_client_str(frame: &ClientFrame) -> Result<String, LumencastError> {
    encode_str_with_envelope(frame)
}

/// Decode a client frame from a UTF-8 JSON string.
pub fn decode_client_str(input: &str) -> Result<ClientFrame, LumencastError> {
    decode_client(input.as_bytes())
}

// --- internals ----------------------------------------------------------

fn encode_with_envelope<T: Serialize>(frame: &T) -> Result<Vec<u8>, LumencastError> {
    let mut value = serde_json::to_value(frame)?;
    inject_version(&mut value)?;
    Ok(serde_json::to_vec(&value)?)
}

fn encode_str_with_envelope<T: Serialize>(frame: &T) -> Result<String, LumencastError> {
    let mut value = serde_json::to_value(frame)?;
    inject_version(&mut value)?;
    Ok(serde_json::to_string(&value)?)
}

fn decode_with_envelope<T: DeserializeOwned>(input: &[u8]) -> Result<T, LumencastError> {
    let raw: Value = serde_json::from_slice(input)?;
    let obj = raw
        .as_object()
        .ok_or_else(|| LumencastError::InvalidEnvelope("frame must be a JSON object".into()))?;

    let v = obj
        .get("v")
        .and_then(Value::as_u64)
        .ok_or_else(|| LumencastError::InvalidEnvelope("envelope missing `v` field".into()))?;

    if v != u64::from(PROTOCOL_VERSION) {
        return Err(LumencastError::VersionMismatch { got: v });
    }

    if !obj.contains_key("type") {
        return Err(LumencastError::InvalidEnvelope(
            "envelope missing `type` field".into(),
        ));
    }

    Ok(serde_json::from_value::<T>(raw)?)
}

fn inject_version(value: &mut Value) -> Result<(), LumencastError> {
    let obj: &mut Map<String, Value> = value.as_object_mut().ok_or_else(|| {
        LumencastError::InvalidEnvelope("frame must serialize to a JSON object".into())
    })?;
    obj.insert("v".to_string(), Value::from(PROTOCOL_VERSION));
    Ok(())
}

fn validate_server(frame: ServerFrame) -> Result<ServerFrame, LumencastError> {
    if let ServerFrame::Delta(d) = &frame {
        validate_patches(&d.patches)?;
    } else if let ServerFrame::Snapshot(s) = &frame {
        for (path, value) in &s.state {
            if value.is_object() {
                return Err(LumencastError::invalid_value(format!(
                    "snapshot state at {path:?}: leaf value MUST NOT be an object"
                )));
            }
        }
    }
    Ok(frame)
}

fn validate_client(frame: ClientFrame) -> Result<ClientFrame, LumencastError> {
    if let ClientFrame::Input(i) = &frame {
        if i.patches.is_empty() {
            return Err(LumencastError::invalid_value(
                "input frame patches MUST NOT be empty",
            ));
        }
        validate_patches(&i.patches)?;
    }
    Ok(frame)
}

fn validate_patches(patches: &[crate::types::Patch]) -> Result<(), LumencastError> {
    for p in patches {
        if !p.is_value_legal() {
            return Err(LumencastError::invalid_value(format!(
                "patch at {}: value MUST NOT be a JSON object",
                p.path
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::*;
    use crate::types::*;
    use serde_json::json;

    fn snap() -> ServerFrame {
        ServerFrame::Snapshot(Snapshot {
            seq: 1,
            scene_id: SceneId::from("s"),
            scene_version: SceneVersion::from(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            ),
            state: [("title".to_string(), json!("hi"))].into_iter().collect(),
            ts: None,
        })
    }

    #[test]
    fn round_trip_snapshot() {
        let bytes = encode_server(&snap()).unwrap();
        let parsed = decode_server(&bytes).unwrap();
        assert_eq!(snap(), parsed);
    }

    #[test]
    fn rejects_missing_v() {
        let raw = br#"{"type":"pong"}"#;
        let err = decode_server(raw).unwrap_err();
        matches!(err, LumencastError::InvalidEnvelope(_));
    }

    #[test]
    fn rejects_wrong_v() {
        let raw = br#"{"v":2,"type":"pong"}"#;
        let err = decode_server(raw).unwrap_err();
        matches!(err, LumencastError::VersionMismatch { got: 2 });
    }

    #[test]
    fn rejects_object_value_in_delta() {
        let raw =
            br#"{"v":1,"type":"delta","seq":2,"patches":[{"path":"x","value":{"nested":true}}]}"#;
        let err = decode_server(raw).unwrap_err();
        matches!(err, LumencastError::InvalidValue { .. });
    }

    #[test]
    fn rejects_empty_input_patches() {
        let raw = br#"{"v":1,"type":"input","patches":[]}"#;
        let err = decode_client(raw).unwrap_err();
        matches!(err, LumencastError::InvalidValue { .. });
    }

    #[test]
    fn ping_pong_round_trip() {
        let p = ClientFrame::Ping(crate::frames::Ping::default());
        let bytes = encode_client(&p).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("\"v\":1"));
        assert!(s.contains("\"type\":\"ping\""));
        let back = decode_client(&bytes).unwrap();
        assert_eq!(back, ClientFrame::Ping(crate::frames::Ping::default()));

        let pong = ServerFrame::Pong(crate::frames::Pong::default());
        let bytes = encode_server(&pong).unwrap();
        let back = decode_server(&bytes).unwrap();
        assert_eq!(back, ServerFrame::Pong(crate::frames::Pong::default()));
    }
}
