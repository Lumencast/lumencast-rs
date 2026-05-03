//! Server-side adapter helpers.
//!
//! Adapters are tasks that read from external sources and write to
//! [`Scene`] leaf paths.
//!
//! - [`spawn_periodic`] — generic periodic tick.
//! - `http_poll` — JSON HTTP polling, gated by the `adapters-http`
//!   feature. See the [`http_poll`](self::http_poll) module when that
//!   feature is enabled.
//! - `websocket_subscribe` — long-lived JSON WebSocket subscription,
//!   gated by the `adapters-ws` feature. See the
//!   [`websocket_subscribe`](self::websocket_subscribe) module when
//!   that feature is enabled.

use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, sleep_until};

use crate::Scene;

#[cfg(feature = "adapters-http")]
pub mod http_poll;
#[cfg(feature = "adapters-ws")]
pub mod websocket_subscribe;

mod flatten;
pub use flatten::flatten_into_pairs;

/// A user-supplied function that produces patches at each tick.
///
/// `(path, value)` pairs returned are emitted as a single atomic delta
/// on the target [`Scene`].
pub type AdapterFn = Box<
    dyn FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<(String, Value)>> + Send>>
        + Send,
>;

/// Spawn a periodic adapter task. The task ends when the supplied
/// `cancel` token receives a value.
///
/// The adapter is scheduled at fixed intervals — drift is compensated
/// by anchoring on `Instant::now()`. If `f` takes longer than `period`,
/// the next call fires immediately.
pub fn spawn_periodic<F, Fut>(
    scene: Scene,
    period: Duration,
    mut f: F,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Vec<(String, Value)>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut next = Instant::now() + period;
        loop {
            tokio::select! {
                () = sleep_until(next) => {
                    let patches = f().await;
                    if !patches.is_empty() && let Err(e) = scene.emit(patches) {
                        tracing::warn!(?e, scene = scene.id().as_str(), "adapter emit failed");
                    }
                    let now = Instant::now();
                    next += period;
                    if next < now {
                        next = now;
                    }
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        break;
                    }
                }
            }
        }
    })
}
