//! Envelope handling — the `v: 1` outer field shared by every LSDP/1
//! frame.
//!
//! The frame structs in [`crate::frames`] do **not** carry the version
//! field; it is added on encode and verified on decode by the routines
//! in [`crate::codec`].

/// LSDP major version this crate implements.
pub const PROTOCOL_VERSION: u8 = 1;

/// WebSocket subprotocol identifier (set in `Sec-WebSocket-Protocol`).
pub const WEBSOCKET_SUBPROTOCOL: &str = "lsdp.v1";
