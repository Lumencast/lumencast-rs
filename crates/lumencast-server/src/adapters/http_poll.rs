//! HTTP polling adapter (LSML 1.0 §9, `kind: "http_poll"`).
//!
//! Polls a JSON endpoint at a fixed interval, flattens the response
//! into leaf-grain patches under a configurable path prefix, and emits
//! a single atomic delta on the target [`Scene`].

use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::Scene;
use crate::adapters::flatten::flatten_into_pairs;

/// Configuration for the HTTP polling adapter.
#[derive(Debug, Clone)]
pub struct HttpPollConfig {
    /// Endpoint URL (must return JSON).
    pub url: String,
    /// Polling interval. Spec example: 200 ms for live scores.
    pub interval: Duration,
    /// Leaf-path prefix the response is flattened under (e.g.
    /// `"match"` → `match.score.home`, `match.score.away`, …).
    pub writes_to: String,
    /// Extra HTTP headers (e.g. `("Authorization", "Bearer …")`).
    pub headers: Vec<(String, String)>,
    /// Per-request timeout (default: 80% of `interval`).
    pub request_timeout: Option<Duration>,
}

/// Spawn the polling task. The returned [`JoinHandle`] terminates when
/// `cancel` flips to `true` (or the channel closes).
pub fn spawn(
    scene: Scene,
    config: HttpPollConfig,
    mut cancel: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let timeout = config
        .request_timeout
        .unwrap_or_else(|| config.interval.mul_f32(0.8));

    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                ?e,
                "http_poll: failed to build reqwest client; adapter disabled"
            );
            return tokio::spawn(async {});
        }
    };

    tokio::spawn(async move {
        let mut next = tokio::time::Instant::now() + config.interval;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(next) => {
                    let pairs = poll_once(&client, &config).await;
                    if !pairs.is_empty()
                        && let Err(e) = scene.emit(pairs) {
                        tracing::warn!(?e, scene = scene.id().as_str(), "http_poll: emit failed");
                    }
                    let now = tokio::time::Instant::now();
                    next += config.interval;
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

async fn poll_once(client: &reqwest::Client, config: &HttpPollConfig) -> Vec<(String, Value)> {
    let mut req = client.get(&config.url);
    for (name, value) in &config.headers {
        req = req.header(name, value);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(?e, url = %config.url, "http_poll: request failed");
            return Vec::new();
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(
            url = %config.url,
            status = resp.status().as_u16(),
            "http_poll: non-success status"
        );
        return Vec::new();
    }
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(?e, url = %config.url, "http_poll: JSON parse failed");
            return Vec::new();
        }
    };
    flatten_into_pairs(&config.writes_to, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Minimal one-shot HTTP server that always responds with the same
    /// JSON body. Used to test the adapter without pulling extra
    /// dependencies.
    async fn one_shot_server(body: String, max_responses: u32) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicU32::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let count = count.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    if count.fetch_add(1, Ordering::SeqCst) + 1 >= max_responses {
                        // Allow callers to bound the lifetime.
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn flattens_response_into_scene() {
        use lumencast_protocol::SceneVersion;

        let body = json!({
            "score": { "home": 3, "away": 1 },
            "minute": 42
        })
        .to_string();
        let addr = one_shot_server(body, 5).await;

        let scene = Scene::new(
            lumencast_protocol::SceneId::from("test"),
            SceneVersion::from("sha256:".to_string() + &"0".repeat(64)),
        );

        let (tx, rx) = watch::channel(false);
        let mut subscriber = scene.subscribe();
        let handle = spawn(
            scene.clone(),
            HttpPollConfig {
                url: format!("http://{addr}/"),
                interval: Duration::from_millis(50),
                writes_to: "match".into(),
                headers: vec![],
                request_timeout: Some(Duration::from_secs(1)),
            },
            rx,
        );

        // Wait for at least one delta.
        let payload = tokio::time::timeout(Duration::from_secs(3), subscriber.recv())
            .await
            .expect("delta within 3s")
            .expect("recv");
        let mut paths: Vec<&str> = payload.patches.iter().map(|p| p.path.as_str()).collect();
        paths.sort_unstable();
        assert!(
            paths.contains(&"match.minute") && paths.contains(&"match.score.home"),
            "unexpected paths: {paths:?}"
        );

        let _ = tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
