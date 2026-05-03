//! WebSocket connection lifecycle.
//!
//! One spawn per connected client. The handler:
//!
//! 1. Validates the `Sec-WebSocket-Protocol: lsdp.v1` subprotocol.
//! 2. Waits for a `subscribe` frame within
//!    [`crate::config::ServerConfig::subscribe_timeout`].
//! 3. Authenticates the token, resolves the target [`Scene`], emits
//!    `snapshot` (`seq=1`).
//! 4. Pumps deltas from the scene broadcast and propagates server-wide
//!    [`ServerEvent::SceneSwap`](crate::server::ServerEvent) events.
//! 5. Validates incoming `input` frames against role+path policy,
//!    rate-limits, and applies them to the scene.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lumencast_protocol::frames::{
    ClientFrame, Delta, ErrorFrame, SceneChanged, ServerFrame, Snapshot, Subscribe,
};
use lumencast_protocol::types::Patch;
use lumencast_protocol::{
    ErrorCode, LumencastError, SceneId, codec,
    envelope::{WEBSOCKET_SUBPROTOCOL, WEBSOCKET_SUBPROTOCOL_V1_1, WEBSOCKET_SUBPROTOCOLS},
};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::auth::Identity;
use crate::scene::Scene;
use crate::server::{ServerEvent, ServerInner};

/// axum route handler for `GET /ws`.
pub(crate) async fn ws_route(
    State(inner): State<Arc<ServerInner>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !has_lsdp_subprotocol(&headers) {
        return (
            StatusCode::UPGRADE_REQUIRED,
            "expected Sec-WebSocket-Protocol: lsdp.v1 or lsdp.v1.1",
        )
            .into_response();
    }

    // LSDP/1.1 preferred, 1.0 fallback. axum's WebSocketUpgrade::protocols
    // performs RFC-6455 preference negotiation against the client's offered
    // list — the first server-advertised protocol the client also offered
    // wins.
    let max = inner.config.max_frame_bytes;
    ws.protocols(WEBSOCKET_SUBPROTOCOLS.iter().copied())
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| async move {
            if let Err(err) = handle(socket, inner).await {
                tracing::debug!(?err, "ws connection ended with error");
            }
        })
}

fn has_lsdp_subprotocol(headers: &HeaderMap) -> bool {
    headers
        .get_all(http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|p| {
            let trimmed = p.trim();
            trimmed == WEBSOCKET_SUBPROTOCOL || trimmed == WEBSOCKET_SUBPROTOCOL_V1_1
        })
}

async fn handle(socket: WebSocket, inner: Arc<ServerInner>) -> Result<(), HandlerError> {
    let mut conn = Connection::new(socket, inner);
    conn.run().await
}

/// Outcome of a single client frame.
enum Action {
    Continue,
    Stop,
}

#[derive(Debug, thiserror::Error)]
enum HandlerError {
    #[error("websocket: {0}")]
    Ws(#[from] axum::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] LumencastError),
    #[error("subscribe timeout")]
    SubscribeTimeout,
    #[error("client closed")]
    Closed,
    #[error("expected text frame")]
    ExpectedText,
    #[error("first frame must be subscribe")]
    NotSubscribe,
}

struct Connection {
    socket: WebSocket,
    inner: Arc<ServerInner>,
    // LSDP/1.1 §18.1.1 — seq is per-scene, not per-connection. Outgoing
    // frames read scene.current_seq() ; the connection no longer carries
    // its own counter.
    /// Set after scene resolution. Errors emitted before resolution
    /// ship at seq=0 ; afterwards they ride this scene's current seq.
    current_scene: Option<Scene>,
    rate: RateBucket,
}

impl Connection {
    fn new(socket: WebSocket, inner: Arc<ServerInner>) -> Self {
        let limit = inner.config.input_rate_per_sec;
        Self {
            socket,
            current_scene: None,
            rate: RateBucket::new(limit),
            inner,
        }
    }

    async fn run(&mut self) -> Result<(), HandlerError> {
        // 1. Subscribe handshake.
        let subscribe = self.recv_subscribe().await?;
        // 2. Authenticate.
        let Ok(identity) = self.inner.auth.authenticate(subscribe.token.as_str()).await else {
            self.send_error(ErrorCode::AuthDenied, "invalid token", false)
                .await?;
            return Ok(());
        };

        // 3. Resolve scene.
        let Some(scene) = self.resolve_scene(&subscribe, &identity) else {
            self.send_error(ErrorCode::SceneNotFound, "scene unknown", false)
                .await?;
            return Ok(());
        };
        self.current_scene = Some(scene.clone());

        // 4. Subscribe before snapshot so the broadcast cannot drop
        //    deltas that fire while we serialize the snapshot.
        let mut deltas_rx = scene.subscribe();
        let mut events_rx = self.inner.events.subscribe();

        // 5. LSDP/1.1 §4.1, §18 — honour since_sequence when the replay
        //    buffer covers the gap. Otherwise fall back to a fresh
        //    snapshot at the current scene seq.
        let mut sent_initial = false;
        if let Some(since) = subscribe.since_sequence
            && since > 0
            && since <= scene.current_seq()
        {
            let slice = scene.replay_since(since);
            if slice.covered {
                for r in slice.records {
                    self.send_delta(r.seq, &r.patches, r.cause).await?;
                }
                sent_initial = true;
            }
        }
        if !sent_initial {
            self.send_snapshot(&scene).await?;
        }

        // 6. Main loop.
        let mut current = scene;
        loop {
            tokio::select! {
                biased;

                msg = self.socket.recv() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => {
                            match self.handle_text_frame(&t, &current, &identity).await? {
                                Action::Continue => {}
                                Action::Stop => return Ok(()),
                            }
                        }
                        Some(Ok(Message::Binary(_))) => {
                            self.send_error(ErrorCode::Internal, "binary frames forbidden", false).await?;
                            return Ok(());
                        }
                        Some(Ok(Message::Ping(p))) => {
                            self.socket.send(Message::Pong(p)).await?;
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Err(e)) => return Err(HandlerError::Ws(e)),
                    }
                }

                ev = events_rx.recv() => {
                    match ev {
                        Ok(ServerEvent::SceneSwap { scene_id }) => {
                            if let Some(new_scene) = self.inner.scene(scene_id.as_str()) {
                                // §18.1.1 — scene_changed advances the OLD scene's
                                // seq one final step ; the snapshot then ships at
                                // the NEW scene's current seq.
                                let prev_seq = current.advance_seq_for_change();
                                self.send_scene_changed(&new_scene, prev_seq).await?;
                                self.send_snapshot(&new_scene).await?;
                                deltas_rx = new_scene.subscribe();
                                self.current_scene = Some(new_scene.clone());
                                current = new_scene;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Lost server events — safer to bounce so the
                            // runtime can resync cleanly.
                            self.send_error(ErrorCode::Internal, "server event stream lagged", true).await?;
                            return Ok(());
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }

                payload = deltas_rx.recv() => {
                    match payload {
                        Ok(p) => {
                            self.send_delta(p.seq, &p.patches, p.cause).await?;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            self.send_error(ErrorCode::Internal, "delta stream lagged", true).await?;
                            return Ok(());
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }
            }
        }
    }

    async fn recv_subscribe(&mut self) -> Result<Subscribe, HandlerError> {
        let timeout = self.inner.config.subscribe_timeout;
        let raw = tokio::time::timeout(timeout, self.socket.recv())
            .await
            .map_err(|_| HandlerError::SubscribeTimeout)?
            .ok_or(HandlerError::Closed)?
            .map_err(HandlerError::Ws)?;

        let text = match raw {
            Message::Text(t) => t,
            Message::Close(_) => return Err(HandlerError::Closed),
            _ => return Err(HandlerError::ExpectedText),
        };

        let frame = codec::decode_client_str(text.as_str())?;
        match frame {
            ClientFrame::Subscribe(s) => Ok(s),
            _ => Err(HandlerError::NotSubscribe),
        }
    }

    fn resolve_scene(&self, sub: &Subscribe, _identity: &Identity) -> Option<Scene> {
        let id: SceneId = if let Some(id) = sub.scene.clone() {
            id
        } else {
            self.inner.active.read().clone()?
        };
        self.inner.scene(id.as_str())
    }

    async fn handle_text_frame(
        &mut self,
        text: &str,
        scene: &Scene,
        identity: &Identity,
    ) -> Result<Action, HandlerError> {
        let frame = match codec::decode_client_str(text) {
            Ok(f) => f,
            Err(LumencastError::InvalidValue { message, .. }) => {
                self.send_error(ErrorCode::InvalidValue, message, true)
                    .await?;
                return Ok(Action::Continue);
            }
            Err(LumencastError::Json(e)) => {
                self.send_error(
                    ErrorCode::InvalidValue,
                    format!("malformed JSON: {e}"),
                    false,
                )
                .await?;
                return Ok(Action::Stop);
            }
            Err(other) => {
                self.send_error(ErrorCode::Internal, other.to_string(), false)
                    .await?;
                return Ok(Action::Stop);
            }
        };

        match frame {
            ClientFrame::Ping(ping) => {
                // LSDP/1.1 §3.5 — echo nonce verbatim if present.
                self.send(&ServerFrame::Pong(lumencast_protocol::frames::Pong {
                    nonce: ping.nonce.clone(),
                }))
                .await?;
            }
            ClientFrame::Subscribe(_) => {
                self.send_error(ErrorCode::Internal, "duplicate subscribe", false)
                    .await?;
                return Ok(Action::Stop);
            }
            ClientFrame::Input(input) => {
                self.handle_input(input, scene, identity).await?;
            }
            ClientFrame::Unsubscribe(_) => {
                // LSDP/1.1 §4.4 — clean teardown. The caller closes the WS
                // immediately on Action::Stop ; no error frame is sent and
                // no data flows after this point.
                return Ok(Action::Stop);
            }
        }
        Ok(Action::Continue)
    }

    async fn handle_input(
        &mut self,
        input: lumencast_protocol::frames::Input,
        scene: &Scene,
        identity: &Identity,
    ) -> Result<(), HandlerError> {
        if !identity.role.can_input() {
            self.send_error(ErrorCode::WriteForbidden, "role cannot send input", true)
                .await?;
            return Ok(());
        }
        if input.patches.is_empty() {
            self.send_error(
                ErrorCode::InvalidValue,
                "input patches MUST NOT be empty",
                true,
            )
            .await?;
            return Ok(());
        }
        if !self.rate.allow() {
            self.send_error(ErrorCode::RateLimit, "input rate limit exceeded", true)
                .await?;
            return Ok(());
        }

        // Atomic validation: if any patch is illegal, reject the whole
        // frame (LSDP/1 §4.2). Order: role → declared-inputs →
        // value-shape. Stops at the first failure and emits an error
        // frame with the offending `path` populated for UNKNOWN_PATH
        // / INVALID_VALUE / WRITE_FORBIDDEN.
        for p in &input.patches {
            if !identity.role.can_write(&p.path, identity.paths.as_deref()) {
                let msg = format!("path {} not writable by role {}", p.path, identity.role);
                self.send_error_with_path(
                    ErrorCode::WriteForbidden,
                    msg,
                    true,
                    Some(p.path.as_str().to_string()),
                )
                .await?;
                return Ok(());
            }
            if let Err(rejection) = scene.check_input_patch(p) {
                self.send_error_with_path(
                    rejection.code,
                    rejection.message,
                    true,
                    Some(rejection.path),
                )
                .await?;
                return Ok(());
            }
            if !p.is_value_legal() {
                let msg = format!("path {} value MUST NOT be a JSON object", p.path);
                self.send_error_with_path(
                    ErrorCode::InvalidValue,
                    msg,
                    true,
                    Some(p.path.as_str().to_string()),
                )
                .await?;
                return Ok(());
            }
        }

        let pairs: Vec<(String, Value)> = input
            .patches
            .into_iter()
            .map(|p| (p.path.into_string(), p.value))
            .collect();

        // LSDP/1.1 §4.2 + §3.2.3 — when the input carries a
        // `client_msg_id`, echo it verbatim into the resulting delta's
        // `cause.input_id` so optimistic-UI clients can correlate the
        // echo with their predicted state. Subject convention is
        // `<role>:<subject>` (subject from token claims when set, else
        // the role itself).
        let cause = input.client_msg_id.as_ref().map(|msg_id| {
            let subject = if identity.subject.is_empty() {
                identity.role.to_string()
            } else {
                identity.subject.clone()
            };
            lumencast_protocol::types::Cause {
                source: format!("{}:{}", identity.role, subject),
                input_id: Some(msg_id.clone()),
            }
        });

        if let Err(e) = scene.emit_with_cause(pairs, cause) {
            self.send_error(ErrorCode::Internal, e.to_string(), true)
                .await?;
        }
        Ok(())
    }

    async fn send_snapshot(&mut self, scene: &Scene) -> Result<(), HandlerError> {
        // LSDP/1.1 §18.1.1 — snapshot ships at the scene's current seq.
        let seq = scene.current_seq();
        let frame = ServerFrame::Snapshot(Snapshot {
            seq,
            scene_id: scene.id().clone(),
            scene_version: scene.version().clone(),
            state: scene.snapshot_state(),
            ts: Some(now_iso()),
        });
        self.send(&frame).await
    }

    async fn send_scene_changed(&mut self, next: &Scene, seq: u64) -> Result<(), HandlerError> {
        let frame = ServerFrame::SceneChanged(SceneChanged {
            seq,
            scene_id: next.id().clone(),
            scene_version: next.version().clone(),
            ts: Some(now_iso()),
            from_scene_id: None,
            transition: None,
        });
        self.send(&frame).await
    }

    async fn send_delta(
        &mut self,
        seq: u64,
        patches: &[Patch],
        cause: Option<lumencast_protocol::types::Cause>,
    ) -> Result<(), HandlerError> {
        let frame = ServerFrame::Delta(Delta {
            seq,
            patches: patches.to_vec(),
            ts: None,
            cause,
        });
        self.send(&frame).await
    }

    async fn send_error(
        &mut self,
        code: ErrorCode,
        message: impl Into<String>,
        recoverable: bool,
    ) -> Result<(), HandlerError> {
        self.send_error_with_path(code, message, recoverable, None)
            .await
    }

    async fn send_error_with_path(
        &mut self,
        code: ErrorCode,
        message: impl Into<String>,
        recoverable: bool,
        path: Option<String>,
    ) -> Result<(), HandlerError> {
        // Errors are connection-scoped events ; they ride the current
        // scene seq without advancing it (§18.1.1). Pre-subscribe errors
        // (auth denied, scene unknown) ship at seq=0 — those don't have
        // an active scene yet.
        let seq = self.current_scene.as_ref().map_or(0, Scene::current_seq);
        let frame = ServerFrame::Error(ErrorFrame {
            seq,
            code,
            message: message.into(),
            recoverable,
            retry_after_ms: None,
            path,
            ts: Some(now_iso()),
        });
        self.send(&frame).await
    }

    async fn send(&mut self, frame: &ServerFrame) -> Result<(), HandlerError> {
        let s = codec::encode_server_str(frame)?;
        self.socket.send(Message::Text(s)).await?;
        Ok(())
    }
}

/// Simple per-second token bucket for input rate-limiting (LSDP/1 §14.3).
struct RateBucket {
    limit: u32,
    used: u32,
    window_start: Instant,
}

impl RateBucket {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            used: 0,
            window_start: Instant::now(),
        }
    }

    fn allow(&mut self) -> bool {
        if self.limit == 0 {
            return true;
        }
        let now = Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.used = 0;
        }
        if self.used >= self.limit {
            return false;
        }
        self.used += 1;
        true
    }
}

fn now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // Pure integer formatting — avoids pulling chrono for v0.1.
    let (year, month, day, hour, minute, second) = epoch_to_calendar(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
fn epoch_to_calendar(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    // Howard Hinnant's `civil_from_days` adapted for u64 epoch
    // seconds. Inputs are bounded (`secs` is wall-clock since
    // 1970-01-01), so the casts are safe.
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (if m <= 2 { y + 1 } else { y }) as i32;
    (year, m, d, hour, minute, second)
}
