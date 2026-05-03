//! WebSocket subscription adapter (LSML 1.0 §9, `kind: "websocket_subscribe"`).
//!
//! Maintains a long-lived WebSocket connection to an external feed,
//! parses each text frame as JSON, flattens it under a configured
//! path prefix, and emits a single delta per frame. On disconnect the
//! adapter reconnects with bounded exponential backoff.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::Scene;
use crate::adapters::flatten::flatten_into_pairs;

/// Configuration for the WebSocket-subscribe adapter.
#[derive(Debug, Clone)]
pub struct WsSubscribeConfig {
    /// Endpoint URL — `ws://` or `wss://`.
    pub url: String,
    /// Leaf-path prefix the incoming JSON is flattened under.
    pub writes_to: String,
    /// Initial backoff after a disconnect. Doubled on each consecutive
    /// failure up to [`Self::backoff_max`]. Default: 250 ms.
    pub backoff_initial: Duration,
    /// Cap on backoff. Default: 30 s.
    pub backoff_max: Duration,
}

impl Default for WsSubscribeConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            writes_to: String::new(),
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Spawn the WebSocket-subscribe task. The returned [`JoinHandle`]
/// terminates when `cancel` flips to `true`.
pub fn spawn(
    scene: Scene,
    config: WsSubscribeConfig,
    cancel: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = config.backoff_initial;
        loop {
            if *cancel.borrow() {
                break;
            }
            match run_once(&scene, &config, cancel.clone()).await {
                Outcome::Cancelled => break,
                Outcome::Disconnected => {
                    tracing::info!(
                        url = %config.url,
                        backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX),
                        "ws_subscribe: reconnecting"
                    );
                    let mut wait_cancel = cancel.clone();
                    tokio::select! {
                        () = tokio::time::sleep(backoff) => {}
                        _ = wait_cancel.changed() => {
                            if *wait_cancel.borrow() { break; }
                        }
                    }
                    backoff = (backoff * 2).min(config.backoff_max);
                }
                Outcome::Connected => {
                    backoff = config.backoff_initial;
                }
            }
        }
    })
}

#[derive(Debug)]
enum Outcome {
    Cancelled,
    Disconnected,
    /// Server connected at least once and produced a frame; reset the
    /// backoff before the next attempt.
    Connected,
}

async fn run_once(
    scene: &Scene,
    config: &WsSubscribeConfig,
    mut cancel: watch::Receiver<bool>,
) -> Outcome {
    let (ws, _) = match tokio_tungstenite::connect_async(&config.url).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(?e, url = %config.url, "ws_subscribe: connect failed");
            return Outcome::Disconnected;
        }
    };
    let (_write, mut read) = ws.split();
    let mut produced_anything = false;

    loop {
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Outcome::Cancelled;
                }
            }
            msg = read.next() => {
                let Some(msg) = msg else { break };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(?e, url = %config.url, "ws_subscribe: read error");
                        break;
                    }
                };
                match msg {
                    Message::Text(text) => {
                        if apply_payload(scene, &config.writes_to, text.as_ref()) {
                            produced_anything = true;
                        }
                    }
                    Message::Binary(_) | Message::Frame(_) => {
                        tracing::debug!("ws_subscribe: ignoring binary/frame");
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(_) => break,
                }
            }
        }
    }

    if produced_anything {
        Outcome::Connected
    } else {
        Outcome::Disconnected
    }
}

fn apply_payload(scene: &Scene, prefix: &str, text: &str) -> bool {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?e, "ws_subscribe: payload not JSON");
            return false;
        }
    };
    let pairs = flatten_into_pairs(prefix, &value);
    if pairs.is_empty() {
        return false;
    }
    if let Err(e) = scene.emit(pairs) {
        tracing::warn!(?e, "ws_subscribe: emit failed");
        return false;
    }
    true
}
