# lumencast-rs

Rust SDK for [Lumencast][lumencast] — implements [LSDP/1][lsdp] (the wire
protocol) and a server kit for pushing typed leaf-grain state to passive
displays over WebSocket.

[lumencast]: https://github.com/Lumencast/lumencast-protocol
[lsdp]: https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSDP-1.md

## Status

Pre-alpha. Tracks the spec at `LSDP/1` and `LSML 1.0`. API will move until
`v1.0.0`.

## Crate matrix

| Crate                                                   | crates.io                  | Purpose                                                                                                       |
| ------------------------------------------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------- |
| [`lumencast-protocol`](crates/lumencast-protocol)       | `lumencast-protocol`       | Pure protocol layer: envelope, frames, codec, leaf paths, errors, LSML bundle parser + content hash. No IO.  |
| [`lumencast-server`](crates/lumencast-server)           | `lumencast-server`         | Server kit on `axum` + `tokio`: scenes, store, auth, role enforcement, WS handler, optional TLS, adapters.   |
| [`lumencast-client`](crates/lumencast-client)           | `lumencast-client`         | Protocol client: connect, gap-detect reconnect with backoff (§7), seamless token rotation (§8), event stream.|
| [`lumencast-conformance`](crates/lumencast-conformance) | not published (test-only)  | Harness that drives an implementation through the conformance suite.                                          |

The protocol crate has zero IO dependencies and compiles to
`wasm32-unknown-unknown` for use in browser/edge runtimes.

## Quickstart

```rust
use lumencast_server::{Server, MapAuthenticator, Role};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut auth = MapAuthenticator::new();
    auth.insert("operator-token", Role::Operator);
    auth.insert("viewer-token",   Role::Viewer);

    let srv = Server::builder()
        .listen("127.0.0.1:4000")
        .auth(auth)
        .build()
        .await?;

    let scene = srv.new_scene("main-stage")?;
    scene.set("show.title", json!("Hello"))?;

    let s = scene.clone();
    tokio::spawn(async move {
        let mut score = 0i64;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            score += 1;
            let _ = s.set("score.home", json!(score));
        }
    });

    srv.run().await
}
```

Full example in [`examples/basic-scoreboard`](examples/basic-scoreboard).

## Optional features

- `lumencast-server/tls` — in-process TLS termination via
  `axum-server` + `rustls`. The calling binary must install a `rustls`
  crypto provider (e.g. `aws-lc-rs` or `ring`) before
  `Server::run_tls`. For most deployments, prefer a reverse proxy.
- `lumencast-server/adapters-http` — periodic JSON HTTP polling
  adapter (LSML `kind: "http_poll"`). Pulls `reqwest`.
- `lumencast-server/adapters-ws` — long-lived JSON WebSocket
  subscription adapter (LSML `kind: "websocket_subscribe"`). Pulls
  `tokio-tungstenite` with TLS.
- `lumencast-server/adapters` — shorthand for both above.

## LSML bundles

`Bundle::parse_str` reads a bundle, validates the `lsml` major version
and shape, and recomputes the canonical SHA-256 content hash via
`compute_content_hash` / `with_computed_version`.
`ServerHandle::register_bundle` registers the scene with the bundle's
defaults seeded into the store and exposes the canonical bytes at
`GET /scenes/:id/:version_hex` with `Cache-Control: public, max-age=…,
immutable`.

## Token rotation

LSDP/1 §8 places token rotation on the runtime: it opens a new
WebSocket with the fresh token, drains the snapshot, then closes the
old socket. The server has nothing extra to do — just provide an
[`Authenticator`] that accepts rotated tokens.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
```

The protocol crate also compiles to WASM:

```sh
cargo check -p lumencast-protocol --target wasm32-unknown-unknown
```

## MSRV

`1.93.0`. Bumped only when a needed feature stabilises; tracked in CI and
declared in the workspace `Cargo.toml`.

## Spec references

- [LSDP/1](https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSDP-1.md) — wire protocol
- [LSML 1.0](https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSML-1.md) — scene format
- [RUNTIME-API](https://github.com/Lumencast/lumencast-protocol/blob/main/spec/RUNTIME-API.md) — runtime contract
- [PERFORMANCE](https://github.com/Lumencast/lumencast-protocol/blob/main/spec/PERFORMANCE.md) — budgets

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
