//! Public [`Client`] handle and builder.

use lumencast_protocol::types::{SceneId, SessionId, Token};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::events::{Event, EventStream};
use crate::manager::{Command, ManagerInit, run as manager_run};

const EVENT_BUFFER: usize = 256;
const COMMAND_BUFFER: usize = 8;

/// Failures raised by the client API.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Builder is missing a required field.
    #[error("client builder is missing: {0}")]
    BuilderMissing(&'static str),

    /// The manager task is gone — the client has been dropped or the
    /// background task panicked.
    #[error("client manager task is no longer running")]
    ManagerGone,

    /// `set_token` failed (the new connection could not be
    /// established).
    #[error("token rotation failed: {0}")]
    SetToken(String),
}

/// LSDP/1 protocol client.
///
/// A `Client` owns a background task that maintains a single live
/// WebSocket and dispatches protocol events through an internal
/// broadcast channel. Subscribe via [`Client::events`] to consume the
/// stream.
///
/// `Client` is `Clone`-able — every clone shares the same underlying
/// connection.
#[derive(Clone)]
pub struct Client {
    cmd_tx: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
}

impl Client {
    /// Start a builder.
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Subscribe to the protocol event stream.
    ///
    /// Each call returns a fresh receiver — multiple consumers can
    /// listen concurrently. If a consumer falls behind the broadcast
    /// channel's buffer, older events are dropped silently. Treat
    /// `Event::Snapshot` as the recovery anchor.
    #[must_use]
    pub fn events(&self) -> EventStream {
        EventStream::new(self.events.subscribe())
    }

    /// Replace the auth token without dropping the active subscription.
    ///
    /// The client opens a parallel WebSocket with the new token, waits
    /// for its `snapshot`, then atomically swaps to it and closes the
    /// previous socket. Per LSDP/1 §8.
    pub async fn set_token(&self, token: impl Into<Token>) -> Result<(), ClientError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::SetToken {
                token: token.into(),
                reply: tx,
            })
            .await
            .map_err(|_| ClientError::ManagerGone)?;
        match rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(ClientError::SetToken(e.to_string())),
            Err(_) => Err(ClientError::ManagerGone),
        }
    }

    /// Close the connection and shut down the background task.
    pub async fn disconnect(&self) -> Result<(), ClientError> {
        self.cmd_tx
            .send(Command::Disconnect)
            .await
            .map_err(|_| ClientError::ManagerGone)
    }
}

/// Builder for [`Client`].
#[derive(Default)]
pub struct ClientBuilder {
    url: Option<String>,
    token: Option<Token>,
    scene: Option<SceneId>,
    session: Option<SessionId>,
    max_reconnect_attempts: Option<u32>,
}

impl ClientBuilder {
    /// WebSocket URL — `ws://` or `wss://`.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Auth token.
    #[must_use]
    pub fn token(mut self, token: impl Into<Token>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Optional scene id (test mode).
    #[must_use]
    pub fn scene(mut self, scene: impl Into<SceneId>) -> Self {
        self.scene = Some(scene.into());
        self
    }

    /// Optional test-session id.
    #[must_use]
    pub fn session(mut self, session: impl Into<SessionId>) -> Self {
        self.session = Some(session.into());
        self
    }

    /// Cap on reconnection attempts. `None` means retry forever (the
    /// LSDP/1 §7 default — never give up but cap the backoff at 60 s).
    #[must_use]
    pub fn max_reconnect_attempts(mut self, max: Option<u32>) -> Self {
        self.max_reconnect_attempts = max;
        self
    }

    /// Build and start the client. The first WS connection is
    /// attempted in the background; the call returns once the manager
    /// task is running.
    #[allow(clippy::unused_async)]
    pub async fn build(self) -> Result<Client, ClientError> {
        let url = self.url.ok_or(ClientError::BuilderMissing("url"))?;
        let token = self.token.ok_or(ClientError::BuilderMissing("token"))?;

        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_BUFFER);
        let (events_tx, _events_rx) = broadcast::channel(EVENT_BUFFER);

        let init = ManagerInit {
            url,
            token,
            scene: self.scene,
            session: self.session,
            max_reconnect_attempts: self.max_reconnect_attempts,
        };
        let events_for_task = events_tx.clone();
        tokio::spawn(async move {
            manager_run(init, cmd_rx, events_for_task).await;
        });

        Ok(Client {
            cmd_tx,
            events: events_tx,
        })
    }
}
