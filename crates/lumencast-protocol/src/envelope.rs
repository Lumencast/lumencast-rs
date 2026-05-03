//! Envelope handling — the `v: 1` outer field shared by every LSDP/1
//! frame.
//!
//! The frame structs in [`crate::frames`] do **not** carry the version
//! field; it is added on encode and verified on decode by the routines
//! in [`crate::codec`].

/// LSDP major version this crate implements.
pub const PROTOCOL_VERSION: u8 = 1;

/// LSDP/1.0 WebSocket subprotocol identifier. Kept for backwards-compatible
/// negotiation with 1.0-only clients.
pub const WEBSOCKET_SUBPROTOCOL: &str = "lsdp.v1";

/// LSDP/1.1 WebSocket subprotocol identifier. Clients advertising this opt
/// into the additive 1.1 frame surface (`since_sequence` resume, `unsubscribe`,
/// per-leaf transition directive, `cause`, `nonce` on ping/pong, `client_msg_id`
/// on input, `from_scene_id` + show transition on `scene_changed`).
pub const WEBSOCKET_SUBPROTOCOL_V1_1: &str = "lsdp.v1.1";

/// Canonical advertise/accept list, ordered by preference (1.1 first, 1.0
/// fallback). Servers MUST advertise both to remain compatible with 1.0
/// clients.
pub const WEBSOCKET_SUBPROTOCOLS: &[&str] = &[WEBSOCKET_SUBPROTOCOL_V1_1, WEBSOCKET_SUBPROTOCOL];
