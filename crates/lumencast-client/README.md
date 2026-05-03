# lumencast-client

[![crates.io](https://img.shields.io/crates/v/lumencast-client.svg)](https://crates.io/crates/lumencast-client)
[![docs.rs](https://docs.rs/lumencast-client/badge.svg)](https://docs.rs/lumencast-client)

LSDP/1 protocol client. Handles the wire-level concerns specified in
LSDP/1 §5–§8:

- WebSocket connect with the `lsdp.v1` subprotocol
- `subscribe` handshake
- Sequence tracking with gap detection → close + reconnect
- Bounded reconnection schedule with ±25% jitter (§7)
- Seamless token rotation: new connection takes over without loss (§8)
- Heartbeat ping/pong

This is **not a runtime** (no rendering, no DOM/native widgets) — it
exposes the protocol stream as a typed Rust [`Stream`] of events. Use
it for integration tests, conformance, programmatic operators,
backend bridges.

## Quickstart

```rust,no_run
use futures_util::StreamExt;
use lumencast_client::{Client, Event};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder()
    .url("ws://127.0.0.1:4000/ws")
    .token("op-token")
    .build()
    .await?;

let mut events = client.events();
while let Some(ev) = events.next().await {
    match ev {
        Event::Snapshot(s) => println!("snapshot {}", s.scene_id),
        Event::Delta(d) => println!("{} patches", d.patches.len()),
        Event::SceneChanged(c) => println!("scene -> {}", c.scene_id),
        Event::Error(e) => println!("error {}: {}", e.code, e.message),
        Event::Status(s) => println!("status: {s:?}"),
    }
}
# Ok(())
# }
```

## Token rotation

```rust,no_run
# use lumencast_client::Client;
# async fn run(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
client.set_token("new-op-token").await?;
# Ok(())
# }
```

The client opens a parallel WebSocket with the new token, drains its
snapshot, then atomically replaces the live connection. The event
stream stays open across the swap.

## License

Apache-2.0.
