# lumencast-protocol

[![crates.io](https://img.shields.io/crates/v/lumencast-protocol.svg)](https://crates.io/crates/lumencast-protocol)
[![docs.rs](https://docs.rs/lumencast-protocol/badge.svg)](https://docs.rs/lumencast-protocol)

Pure protocol layer for [LSDP/1][lsdp] — Lumencast's wire format. No IO. No
async. Compiles to `wasm32-unknown-unknown`.

[lsdp]: https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSDP-1.md

## Modules

| Module      | Purpose                                                              |
| ----------- | -------------------------------------------------------------------- |
| `envelope`  | Outer envelope: `v`, `type`, optional `seq` and `ts`.                |
| `codec`     | JSON encode/decode for whole frames (text-frame payloads).           |
| `frames`    | `FrameKind` enum + per-variant typed shapes for every LSDP/1 frame.  |
| `sequence`  | Server-side sequence allocator and client-side gap-detecting tracker.|
| `leaf_path` | `LeafPath` newtype with parsing, scope substitution, namespace tags. |
| `errors`    | `ErrorCode` enum (closed taxonomy) and `LumencastError` variants.    |
| `types`     | Shared types: `LeafValue`, `Patch`, `SceneId`, `SceneVersion`.       |

## Example

```rust
use lumencast_protocol::frames::{ServerFrame, Snapshot};
use lumencast_protocol::types::SceneVersion;
use lumencast_protocol::codec;
use serde_json::json;

let frame = ServerFrame::Snapshot(Snapshot {
    seq: 1,
    scene_id: "main-stage".into(),
    scene_version: SceneVersion::new("sha256:abc123".into()),
    state: [("show.title".into(), json!("Hello"))].into_iter().collect(),
    ts: None,
});

let bytes = codec::encode_server(&frame).unwrap();
let parsed = codec::decode_server(&bytes).unwrap();
assert_eq!(frame, parsed);
```

## License

Apache-2.0.
