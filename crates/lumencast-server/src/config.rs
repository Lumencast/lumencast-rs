//! Server tunables.

use std::time::Duration;

/// Server configuration. Defaults are spec-aligned (LSDP/1 §14).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Maximum size of an incoming WebSocket frame, in bytes. Larger
    /// frames are rejected with `INVALID_VALUE` and the connection is
    /// closed (LSDP/1 §14.3). Default: 64 KiB.
    pub max_frame_bytes: usize,

    /// Maximum `input` frames per second per connection (LSDP/1 §14.3).
    /// Default: 60.
    pub input_rate_per_sec: u32,

    /// Period at which the server responds to client `ping`s and
    /// optionally emits its own (LSDP/1 §12). Default: 30 s.
    pub heartbeat: Duration,

    /// How long the server waits for the first `subscribe` frame after
    /// the WebSocket upgrade succeeds. Default: 5 s.
    pub subscribe_timeout: Duration,

    /// Default test session TTL (LSDP/1 §11). Default: 5 min.
    pub test_session_ttl: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 64 * 1024,
            input_rate_per_sec: 60,
            heartbeat: Duration::from_secs(30),
            subscribe_timeout: Duration::from_secs(5),
            test_session_ttl: Duration::from_mins(5),
        }
    }
}
