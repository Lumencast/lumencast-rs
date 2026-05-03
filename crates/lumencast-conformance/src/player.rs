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
use crate::scenario::{Scenario, Step};

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

    // 3. /test/setup. Mirrors the Go SDK's HTTPDriver.bundlesFor :
    // when scenarios declare bundles use them (with inline.scene_id
    // overriding bundle.id), otherwise synthesise a minimal bundle
    // from the first server-sends snapshot. Same for initial_state —
    // either declared or extracted from the first snapshot.
    let bundles = build_setup_bundles(scenario, &bundle_hashes);
    let initial_state = if scenario.initial_state.is_empty() {
        extract_initial_state(scenario)
    } else {
        scenario.initial_state.clone()
    };
    let setup = SetupRequest {
        scenario: scenario.name.clone(),
        tokens: tokens.clone(),
        bundles,
        initial_state,
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
            // SCENARIO-FORMAT.md `expect-no-frame-for` § Connection-close
            // semantics : a clean server-initiated close (codes 1000 /
            // 1001 / 1005) within the duration is success — the
            // conceptual contract is "no data flowed". Abnormal closures
            // remain failures.
            let result = tokio::time::timeout(Duration::from_millis(*duration_ms), ws.next()).await;
            match result {
                // Timeout, or stream ended without a close frame — both
                // count as success (no data flowed).
                Err(_) | Ok(None) => {}
                Ok(Some(Ok(Message::Close(close_frame)))) => {
                    // RFC-6455 close codes : 1000 NormalClosure,
                    // 1001 GoingAway, 1005 NoStatus all qualify as clean.
                    let code = close_frame.as_ref().map_or(1005, |cf| u16::from(cf.code));
                    if !matches!(code, 1000 | 1001 | 1005) {
                        return Err(PlayerError::StepFailed {
                            step_index: idx,
                            message: format!(
                                "expect-no-frame-for: abnormal close code {code} within {duration_ms} ms"
                            ),
                        });
                    }
                }
                Ok(Some(_)) => {
                    return Err(PlayerError::StepFailed {
                        step_index: idx,
                        message: format!(
                            "expect-no-frame-for: a frame arrived within {duration_ms} ms"
                        ),
                    });
                }
            }
        }
        Step::ExpectClientAction(action) => match action.action.as_str() {
            "close-with-reason" => {
                let reason = action
                    .fields
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let resolved = subs.apply(&Value::String(reason.to_string()));
                let want = resolved.as_str().unwrap_or(reason).to_string();
                let frame = ws.next().await;
                match frame {
                    Some(Ok(Message::Close(Some(CloseFrame { reason: got, .. })))) => {
                        // $ANY accepts any reason — useful for
                        // scenarios that only assert "closed", not
                        // "closed with this exact text".
                        if want != "$ANY" && !got.contains(&want) {
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
            "reconnect" => {
                // Harness can't observe the runtime opening a fresh
                // connection from outside; this step is a no-op when
                // the harness IS the client.
            }
            other => {
                // Unsupported runtime-target action (fetch-bundle,
                // onError, ...). Scenarios that need these are
                // target=runtime and should have been auto-skipped
                // upstream — if we land here it's a scenario shape
                // bug.
                return Err(PlayerError::StepFailed {
                    step_index: idx,
                    message: format!(
                        "expect-client-action: action {other:?} not supported in server-target harness"
                    ),
                });
            }
        },
        Step::ServerEmits { frame } => {
            // Extract patches from the expected frame, POST /test/emit,
            // then read the actual frame and structurally match — same
            // semantic as `server-sends` but harness-orchestrated.
            // Currently only `frame.type == "delta"` is supported per
            // SCENARIO-FORMAT.md § server-emits.
            let frame_type = frame.get("type").and_then(Value::as_str).unwrap_or("");
            if frame_type != "delta" {
                return Err(PlayerError::StepFailed {
                    step_index: idx,
                    message: format!(
                        "server-emits only supports type=delta today, got {frame_type:?}"
                    ),
                });
            }
            let raw_patches = frame
                .get("patches")
                .and_then(Value::as_array)
                .ok_or_else(|| PlayerError::StepFailed {
                    step_index: idx,
                    message: "server-emits delta missing `patches` list".into(),
                })?;
            let mut pairs: Vec<(String, Value)> = Vec::with_capacity(raw_patches.len());
            for p in raw_patches {
                let path = p
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| PlayerError::StepFailed {
                        step_index: idx,
                        message: "server-emits patch missing `path`".into(),
                    })?
                    .to_string();
                let value = p
                    .get("value")
                    .cloned()
                    .ok_or_else(|| PlayerError::StepFailed {
                        step_index: idx,
                        message: "server-emits patch missing `value`".into(),
                    })?;
                pairs.push((path, subs.apply(&value)));
            }
            control.emit(pairs).await?;
            // Read + match the resulting wire frame.
            let actual = read_text_frame(ws, idx, Duration::from_secs(5)).await?;
            update_shadow_from_frame(&actual, shadow_state);
            let expected = subs.apply(frame);
            if let Err(diff) = structural_match(&expected, &actual) {
                return Err(PlayerError::StepFailed {
                    step_index: idx,
                    message: format!("server-emits mismatch: {diff} (got {actual})"),
                });
            }
        }
        Step::Unsupported { kind, .. } => {
            // We intentionally accept unknown step kinds at parse
            // time so target=runtime scenarios can be loaded and
            // skipped. If a server-target scenario reaches one of
            // these, surface it as a clear runtime error.
            return Err(PlayerError::StepFailed {
                step_index: idx,
                message: format!("step kind {kind:?} not supported by server-target harness"),
            });
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
///
/// Sentinels in the expected template :
/// - `$ANY` — matches any value of any type
/// - `$ANY_HASH` — matches any sha256:<hex64> string
fn structural_match(expected: &Value, actual: &Value) -> Result<(), String> {
    if let Some(s) = expected.as_str() {
        match s {
            "$ANY" => return Ok(()),
            "$ANY_HASH" => {
                let actual_str = actual
                    .as_str()
                    .ok_or_else(|| format!("$ANY_HASH expects a string, got {actual}"))?;
                if !is_sha256_hash(actual_str) {
                    return Err(format!("$ANY_HASH does not match {actual_str:?}"));
                }
                return Ok(());
            }
            _ => {}
        }
    }
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

/// Lightweight sha256 hash check : prefix `sha256:` + 64 hex chars.
fn is_sha256_hash(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

impl Substitutions {
    /// Apply substitutions to every value of a `String → Value` map.
    fn apply_to_map(&self, map: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        map.iter()
            .map(|(k, v)| (k.clone(), self.apply(v)))
            .collect()
    }
}

/// Mirror of Go's `HTTPDriver.bundlesFor` : pick the right (id, hash,
/// inline) for `/test/setup`, synthesising a minimal bundle when the
/// scenario doesn't declare one.
fn build_setup_bundles(
    scenario: &Scenario,
    bundle_hashes: &BTreeMap<String, String>,
) -> Vec<SetupBundle> {
    if !scenario.bundles.is_empty() {
        return scenario
            .bundles
            .iter()
            .map(|b| {
                // inline.scene_id wins over bundle.id — the bundle's id
                // is the scenario-local reference name, the LSML's
                // scene_id is the server-side scene identifier. Without
                // this, scenarios that expect a specific scene_id (e.g.
                // "t") get rejected when their bundle's wrapper id is
                // different (e.g. "title-bundle").
                let id = b
                    .inline
                    .get("scene_id")
                    .and_then(Value::as_str)
                    .unwrap_or(&b.id)
                    .to_string();
                SetupBundle {
                    id,
                    hash: bundle_hashes
                        .get(&b.id)
                        .cloned()
                        .unwrap_or_else(|| "sha256:".to_string()),
                    inline: Some(b.inline.clone()),
                }
            })
            .collect();
    }

    // Synthetic bundle for scenarios with no explicit declaration.
    let (id, hash) = first_scene_id_and_hash(scenario);
    let id = if id.is_empty() {
        scenario.name.clone()
    } else {
        id
    };
    let hash = if hash.is_empty() {
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string()
    } else {
        hash
    };
    // Synthesise operator_inputs from the snapshot's __inputs.* state
    // keys so the server enforces path declaredness.
    let initial_state = extract_initial_state(scenario);
    let mut inline = serde_json::Map::new();
    let mut inputs = Vec::new();
    for path in initial_state.keys() {
        if path.starts_with("__inputs.") {
            inputs.push(serde_json::json!({"path": path}));
        }
    }
    if !inputs.is_empty() {
        inline.insert("operator_inputs".into(), Value::Array(inputs));
    }

    vec![SetupBundle {
        id,
        hash,
        inline: Some(Value::Object(inline)),
    }]
}

/// Mirror of Go's `firstSceneIDAndHash` : scan server-sends steps for
/// a literal `scene_id` + `scene_version`. Empty strings if not found
/// (which means the harness will fall back to placeholder defaults).
fn first_scene_id_and_hash(scenario: &Scenario) -> (String, String) {
    let mut id = String::new();
    let mut hash = String::new();
    for step in &scenario.steps {
        let Step::ServerSends { frame } = step else {
            continue;
        };
        if id.is_empty()
            && let Some(v) = frame.get("scene_id").and_then(Value::as_str)
            && v != "$ANY"
        {
            id = v.to_string();
        }
        if hash.is_empty()
            && let Some(v) = frame.get("scene_version").and_then(Value::as_str)
            && v != "$ANY_HASH"
        {
            hash = v.to_string();
        }
        if !id.is_empty() && !hash.is_empty() {
            break;
        }
    }
    (id, hash)
}

/// Mirror of Go's `extractInitialState` : pull state from the first
/// server-sends snapshot, dropping `$ANY*` sentinels which cannot be
/// seeded literally.
fn extract_initial_state(scenario: &Scenario) -> BTreeMap<String, Value> {
    for step in &scenario.steps {
        let Step::ServerSends { frame } = step else {
            continue;
        };
        if frame.get("type").and_then(Value::as_str) != Some("snapshot") {
            continue;
        }
        let Some(state) = frame.get("state").and_then(Value::as_object) else {
            continue;
        };
        let mut out = BTreeMap::new();
        for (k, v) in state {
            if let Some(s) = v.as_str()
                && (s == "$ANY" || s == "$ANY_HASH")
            {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
        return out;
    }
    BTreeMap::new()
}
