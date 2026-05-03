//! Public event types emitted by [`crate::Client`].

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use lumencast_protocol::frames::{Delta, ErrorFrame, SceneChanged, Snapshot};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Connection status, mirroring the runtime API contract
/// (`RUNTIME-API.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Not connected and not currently connecting.
    Disconnected,
    /// WebSocket open or `subscribe` in flight.
    Connecting,
    /// `snapshot` received, deltas flowing.
    Live,
}

/// Event emitted by [`crate::Client`].
#[derive(Debug, Clone)]
pub enum Event {
    /// Connection status changed.
    Status(Status),
    /// Server emitted a `snapshot` (initial or after `scene_changed`).
    Snapshot(Snapshot),
    /// Server emitted a `delta`.
    Delta(Delta),
    /// Active scene was swapped server-side. The next event will be a
    /// fresh `snapshot`.
    SceneChanged(SceneChanged),
    /// Server emitted an `error` frame.
    Error(ErrorFrame),
}

/// Stream of [`Event`]s produced by [`crate::Client::events`].
///
/// Built on top of a Tokio broadcast channel: if the consumer falls
/// far behind the producer, older events are dropped silently. For
/// authoritative state, listen for [`Event::Snapshot`] and rebuild
/// from there.
pub struct EventStream {
    inner: BroadcastStream<Event>,
}

impl EventStream {
    pub(crate) fn new(rx: broadcast::Receiver<Event>) -> Self {
        Self {
            inner: BroadcastStream::new(rx),
        }
    }
}

impl Stream for EventStream {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(ev))) => return Poll::Ready(Some(ev)),
                Poll::Ready(Some(Err(_))) => {} // skip lagged, loop
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
