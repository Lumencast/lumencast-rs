//! Background task that owns the live WebSocket and dispatches
//! protocol events.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lumencast_protocol::frames::ServerFrame;
use lumencast_protocol::types::{SceneId, SessionId, Token};
use lumencast_protocol::{SequenceTracker, codec};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::backoff::backoff_for_attempt;
use crate::connection::{ConnectError, Connection};
use crate::events::{Event, Status};

/// Commands the [`crate::Client`] handle sends to the manager task.
pub(crate) enum Command {
    SetToken {
        token: Token,
        reply: oneshot::Sender<Result<(), ConnectError>>,
    },
    Disconnect,
}

pub(crate) struct ManagerInit {
    pub(crate) url: String,
    pub(crate) token: Token,
    pub(crate) scene: Option<SceneId>,
    pub(crate) session: Option<SessionId>,
    pub(crate) max_reconnect_attempts: Option<u32>,
}

pub(crate) async fn run(
    init: ManagerInit,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
) {
    let mut state = State {
        url: init.url,
        token: init.token,
        scene: init.scene,
        session: init.session,
        max_reconnect_attempts: init.max_reconnect_attempts,
    };

    'outer: loop {
        // Connect with retry.
        let conn = match connect_with_retry(&state, &events, &mut commands).await {
            ConnectOutcome::Connected(c) => *c,
            ConnectOutcome::Cancelled => break 'outer,
            ConnectOutcome::Exhausted => {
                let _ = events.send(Event::Status(Status::Disconnected));
                break 'outer;
            }
        };

        if matches!(
            drive(conn, &mut state, &events, &mut commands).await,
            DriveOutcome::Disconnect
        ) {
            break 'outer;
        }
    }

    let _ = events.send(Event::Status(Status::Disconnected));
}

struct State {
    url: String,
    token: Token,
    scene: Option<SceneId>,
    session: Option<SessionId>,
    max_reconnect_attempts: Option<u32>,
}

enum ConnectOutcome {
    Connected(Box<Connection>),
    Cancelled,
    Exhausted,
}

enum DriveOutcome {
    Disconnect,
    Reconnect,
}

async fn connect_with_retry(
    state: &State,
    events: &broadcast::Sender<Event>,
    commands: &mut mpsc::Receiver<Command>,
) -> ConnectOutcome {
    let mut attempt = 1u32;
    loop {
        let _ = events.send(Event::Status(Status::Connecting));
        match Connection::open(
            &state.url,
            &state.token,
            state.scene.as_ref(),
            state.session.as_ref(),
            None,
        )
        .await
        {
            Ok(conn) => {
                let _ = events.send(Event::Status(Status::Live));
                let _ = events.send(Event::Snapshot(conn.snapshot.clone()));
                return ConnectOutcome::Connected(Box::new(conn));
            }
            Err(e) => {
                tracing::warn!(?e, attempt, "connection attempt failed");
                if let Some(max) = state.max_reconnect_attempts
                    && attempt >= max
                {
                    return ConnectOutcome::Exhausted;
                }
                let delay = backoff_for_attempt(attempt + 1);
                if delay > Duration::ZERO {
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        cmd = commands.recv() => {
                            if matches!(cmd, None | Some(Command::Disconnect)) {
                                return ConnectOutcome::Cancelled;
                            }
                            // Other commands during retry: ignore here;
                            // they'll be processed once connected.
                        }
                    }
                }
                attempt += 1;
            }
        }
    }
}

async fn drive(
    mut conn: Connection,
    state: &mut State,
    events: &broadcast::Sender<Event>,
    commands: &mut mpsc::Receiver<Command>,
) -> DriveOutcome {
    let mut tracker = SequenceTracker::new();
    let _ = tracker.observe_seq(conn.snapshot.seq);

    loop {
        tokio::select! {
            biased;

            cmd = commands.recv() => {
                match cmd {
                    Some(Command::Disconnect) | None => {
                        let _ = conn.socket.close(None).await;
                        return DriveOutcome::Disconnect;
                    }
                    Some(Command::SetToken { token, reply }) => {
                        match swap_token(&mut conn, state, token, events).await {
                            Ok(()) => {
                                tracker = SequenceTracker::new();
                                let _ = tracker.observe_seq(conn.snapshot.seq);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                }
            }

            msg = conn.socket.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        match codec::decode_server_str(t.as_ref()) {
                            Ok(frame) => {
                                if let Err(reason) = handle_frame(frame, &mut tracker, events) {
                                    tracing::info!(?reason, "reconnecting");
                                    let _ = conn.socket.close(None).await;
                                    return DriveOutcome::Reconnect;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(?e, "decode failed; dropping connection");
                                let _ = conn.socket.close(None).await;
                                return DriveOutcome::Reconnect;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = conn.socket.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_) | Message::Binary(_) | Message::Frame(_))) | None => {
                        return DriveOutcome::Reconnect;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(?e, "websocket read error");
                        return DriveOutcome::Reconnect;
                    }
                }
            }
        }
    }
}

/// Reason returned by [`handle_frame`] when the connection must be
/// recycled.
#[derive(Debug)]
enum FrameDispatch {
    /// Sequence gap detected — close and reconnect (LSDP/1 §5).
    Gap,
}

fn handle_frame(
    frame: ServerFrame,
    tracker: &mut SequenceTracker,
    events: &broadcast::Sender<Event>,
) -> Result<(), FrameDispatch> {
    use lumencast_protocol::sequence::Observation;
    if let Some(seq) = frame.seq() {
        match tracker.observe_seq(seq) {
            Ok(Observation::Accepted | Observation::Heartbeat) => {}
            Ok(Observation::Duplicate) => return Ok(()),
            Err(_) => return Err(FrameDispatch::Gap),
        }
    }
    match frame {
        ServerFrame::Snapshot(s) => {
            let _ = events.send(Event::Snapshot(s));
        }
        ServerFrame::Delta(d) => {
            let _ = events.send(Event::Delta(d));
        }
        ServerFrame::SceneChanged(c) => {
            tracker.reset();
            let _ = events.send(Event::SceneChanged(c));
        }
        ServerFrame::Error(e) => {
            let _ = events.send(Event::Error(e));
        }
        ServerFrame::Pong => {}
    }
    Ok(())
}

async fn swap_token(
    conn: &mut Connection,
    state: &mut State,
    new_token: Token,
    events: &broadcast::Sender<Event>,
) -> Result<(), ConnectError> {
    // Open a parallel connection with the new token.
    let new_conn = Connection::open(
        &state.url,
        &new_token,
        state.scene.as_ref(),
        state.session.as_ref(),
        None,
    )
    .await?;
    // Replace, drop the old socket (its drop closes the WS).
    let old = std::mem::replace(conn, new_conn);
    drop(old.socket);
    state.token = new_token;
    let _ = events.send(Event::Snapshot(conn.snapshot.clone()));
    Ok(())
}
