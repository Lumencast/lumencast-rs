# lumencast-server

[![crates.io](https://img.shields.io/crates/v/lumencast-server.svg)](https://crates.io/crates/lumencast-server)
[![docs.rs](https://docs.rs/lumencast-server/badge.svg)](https://docs.rs/lumencast-server)

Server kit for [Lumencast][lumencast] on `axum` + `tokio`. Implements
LSDP/1 server semantics: scene state, leaf-grain deltas, role
enforcement, sequenced fan-out, scene swap.

[lumencast]: https://github.com/Lumencast/lumencast-protocol

## Quickstart

```rust
use lumencast_server::{MapAuthenticator, Role, Server};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut auth = MapAuthenticator::new();
    auth.insert("op-token", Role::Operator);
    auth.insert("viewer",   Role::Viewer);

    let srv = Server::builder()
        .listen("127.0.0.1:4000")
        .auth(auth)
        .build()
        .await?;

    let scene = srv.new_scene("main-stage")?;
    scene.set("show.title", json!("Hello"))?;
    srv.set_active_scene(scene.id().clone())?;

    srv.run().await
}
```

## License

Apache-2.0.
