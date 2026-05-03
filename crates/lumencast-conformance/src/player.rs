//! Step interpreter — runs one [`Scenario`] against a live server.

#![allow(missing_docs)]

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lumencast_protocol::envelope::WEBSOCKET_SUBPROTOCOL;
use serde_json::Value;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::control::{ControlClient, SetupBundle, SetupRequest};
use crate::placeholders::{Substitutions, hash_inline_bundle};
use crate::scenario::{ClientAction, Scenario, Step};

/// Failure raised while running a scenario.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("control: {0}")]
    Control(#[from] crate::control::ControlError),
    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("step {step_index}: {message}")]
    StepFailed { step_index: usize, message: String },
    #[error("ws connection closed unexpectedly")]
    Closed,
    #[error("expected text frame, got {0}")]
    ExpectedText(String),
}

/// Run a single scenario end-to-end. Returns `Ok(())` on success.
pub async fn run_scenario(
    scenario: &Scenario,
    control: &ControlClient,
    tokens: &BTreeMap<String, String>,
) -> Result<(), PlayerError> {
    // 1. Compute bundle hashes.
    let bundle_hashes: BTreeMap<String, String> = scenario
        .bundles
        .iter()
        .map(|b| (b.id.clone(), hash_inline_bundle(&b.inline)))
        .collect();

    // 2. Substitutions.
    let subs = Substitutions::new(tokens, &bundle_hashes);

    // 3. /test/setup.
    let setup = SetupRequest {
        scenario: scenario.name.clone(),
        tokens: tokens.clone(),
        bundles: scenario
            .bundles
            .iter()
            .map(|b| SetupBundle {
                id: b.id.clone(),
                hash: bundle_hashes
                    .get(&b.id)
                    .cloned()
                    .unwrap_or_else(|| "sha256:".to_string()),
                inline: Some(b.inline.clone()),
            })
            .collect(),
        initial_state: scenario.initial_state.clone(),
    };
    let setup_resp = control.setup(&setup).await?;

    // 4. Open WS.
    let mut ws = open_ws(&setup_resp.ws_url).await?;

    // Shadow state rebuilt from observed snapshot/delta frames.
    let mut shadow_state: BTreeMap<String, Value> = BTreeMap::new();

    // 5. Step interpreter.
    for (idx, step) in scenario.steps.iter().enumerate() {
        run_step(idx, step, &subs, &mut ws, &mut shadow_state, control).await?;
    }

    // 6. /test/reset between scenarios.
    control.reset().await?;
    Ok(())
}

async fn open_ws(
    url: &str,
) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, PlayerError> {
    let request = build_request(url).map_err(|_e| PlayerError::Closed)?;
    let (ws, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(ws)
}

fn build_request(url: &str) -> Result<Request<()>, http::uri::InvalidUri> {
    let uri: http::Uri = url.parse()?;
    let host = uri.host().unwrap_or("localhost").to_string();
    let host_header = match uri.port_u16() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    Ok(Request::builder()
        .method("GET")
        .uri(url)
        .header("Host", host_header)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Protocol", WEBSOCKET_SUBPROTOCOL)
        .body(())
        .expect("request"))
}

#[allow(clippy::too_many_lines)]
async fn run_step(
    idx: usize,
    step: &Step,
    subs: &Substitutions,
    ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    shadow_state: &mut BTreeMap<String, Value>,
    control: &ControlClient,
) -> Result<(), PlayerError> {
    match step {
        Step::ClientSends { frame } => {
            let resolved = subs.apply(frame);
            let text = serde_json::to_string(&resolved)?;
            ws.send(Message::Text(text)).await?;
        }
        Step::ServerSends { frame } => {
            let expected = subs.apply(frame);
            let received = read_text_frame(ws, idx, Duration::from_secs(5)).await?;
            update_shadow_from_frame(&received, shadow_state);
            if let Err(diff) = structural_match(&expected, &received) {
                return Err(PlayerError::StepFailed {
                    step_index: idx,
                    message: format!("server-sends mismatch: {diff}"),
                });
            }
        }
        Step::ExpectRuntimeState { state } => {
            let expected = subs.apply_to_map(state);
            for (k, v) in &expected {
                let Some(actual) = shadow_state.get(k) else {
                    return Err(PlayerError::StepFailed {
                        step_index: idx,
                        message: format!("expect-runtime-state: missing path {k:?}"),
                    });
                };
                if actual != v {
                    return Err(PlayerError::StepFailed {
                        step_index: idx,
                        message: format!(
                            "expect-runtime-state: path {k:?}: expected {v:?}, got {actual:?}"
                        ),
                    });
                }
            }
        }
        Step::ExpectServerState { state } => {
            let expected = subs.apply_to_map(state);
            let resp = control.state().await?;
            for (k, v) in &expected {
                let Some(actual) = resp.state.get(k) else {
                    return Err(PlayerError::StepFailed {
                        step_index: idx,
                        message: format!("expect-server-state: missing path {k:?}"),
                    });
                };
                if actual != v {
                    return Err(PlayerError::StepFailed {
                        step_index: idx,
                        message: format!(
                            "expect-server-state: path {k:?}: expected {v:?}, got {actual:?}"
                        ),
                    });
                }
            }
        }
        Step::ExpectNoFrameFor { duration_ms } => {
            let result = tokio::time::timeout(Duration::from_millis(*duration_ms), ws.next()).await;
            if result.is_ok() {
                return Err(PlayerError::StepFailed {
                    step_index: idx,
                    message: format!(
                        "expect-no-frame-for: a frame arrived within {duration_ms} ms"
                    ),
                });
            }
        }
        Step::ExpectClientAction(action) => match action {
            ClientAction::CloseWithReason { reason } => {
                let resolved = subs.apply(&Value::String(reason.clone()));
                let want = resolved.as_str().unwrap_or(reason).to_string();
                let frame = ws.next().await;
                match frame {
                    Some(Ok(Message::Close(Some(CloseFrame { reason: got, .. })))) => {
                        if !got.contains(&want) {
                            return Err(PlayerError::StepFailed {
                                step_index: idx,
                                message: format!(
                                    "expect-client-action close-with-reason: want {want:?}, got {got:?}"
                                ),
                            });
                        }
                    }
                    Some(Ok(other)) => {
                        return Err(PlayerError::StepFailed {
                            step_index: idx,
                            message: format!(
                                "expect-client-action close-with-reason: got {other:?}"
                            ),
                        });
                    }
                    Some(Err(e)) => return Err(PlayerError::Ws(e)),
                    None => return Err(PlayerError::Closed),
                }
            }
            ClientAction::Reconnect => {
                // Harness can't observe the runtime opening a fresh
                // connection from outside; this step is a no-op when
                // the harness IS the client.
            }
        },
        Step::ServerEmits { patches } => {
            let pairs: Vec<(String, Value)> = patches
                .iter()
                .map(|p| (p.path.clone(), subs.apply(&p.value)))
                .collect();
            control.emit(pairs).await?;
        }
    }
    Ok(())
}

async fn read_text_frame(
    ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    step: usize,
    timeout: Duration,
) -> Result<Value, PlayerError> {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .map_err(|_| PlayerError::StepFailed {
            step_index: step,
            message: format!("server-sends: no frame within {} ms", timeout.as_millis()),
        })?
        .ok_or(PlayerError::Closed)??;
    match msg {
        Message::Text(t) => Ok(serde_json::from_str(t.as_ref())?),
        other => Err(PlayerError::ExpectedText(format!("{other:?}"))),
    }
}

/// Update the shadow store from an observed snapshot or delta frame.
fn update_shadow_from_frame(frame: &Value, shadow: &mut BTreeMap<String, Value>) {
    let Some(obj) = frame.as_object() else { return };
    let frame_type = obj.get("type").and_then(Value::as_str);
    match frame_type {
        Some("snapshot") => {
            shadow.clear();
            if let Some(state) = obj.get("state").and_then(Value::as_object) {
                for (k, v) in state {
                    shadow.insert(k.clone(), v.clone());
                }
            }
        }
        Some("delta") => {
            if let Some(patches) = obj.get("patches").and_then(Value::as_array) {
                for patch in patches {
                    if let (Some(path), Some(value)) = (
                        patch.get("path").and_then(Value::as_str),
                        patch.get("value"),
                    ) {
                        shadow.insert(path.to_string(), value.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

/// Structural match: every key in `expected` must be present in
/// `actual` with the same value (recursively for objects). `actual`
/// MAY have extra keys.
fn structural_match(expected: &Value, actual: &Value) -> Result<(), String> {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (k, ev) in e {
                let Some(av) = a.get(k) else {
                    return Err(format!("missing key {k:?}"));
                };
                structural_match(ev, av).map_err(|inner| format!("{k}: {inner}"))?;
            }
            Ok(())
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                return Err(format!("array length mismatch: {} vs {}", e.len(), a.len()));
            }
            for (i, (ev, av)) in e.iter().zip(a.iter()).enumerate() {
                structural_match(ev, av).map_err(|inner| format!("[{i}]: {inner}"))?;
            }
            Ok(())
        }
        (e, a) if e == a => Ok(()),
        (e, a) => Err(format!("expected {e}, got {a}")),
    }
}

impl Substitutions {
    /// Apply substitutions to every value of a `String → Value` map.
    fn apply_to_map(&self, map: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        map.iter()
            .map(|(k, v)| (k.clone(), self.apply(v)))
            .collect()
    }
}
