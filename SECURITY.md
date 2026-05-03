# Security policy

## Reporting a vulnerability

Please do **not** open a public issue for security reports.

Email the maintainers at `security@lumencast.dev` with:

- A description of the issue
- Steps to reproduce
- The version (crate + git rev) affected
- Your suggested fix, if any

We acknowledge within 72 hours and aim for a fix within 14 days for
high-severity issues.

## Supported versions

Until `v1.0.0`, only the latest published `0.x` minor is supported. After
`v1.0.0`, security patches are backported to the previous minor for 6
months.

## Threat model

`lumencast-rs` is server-side. It accepts WebSocket connections from
untrusted clients. The protocol crate is pure logic and has no IO surface.
The server crate has the following trust boundaries:

- **Untrusted**: the WebSocket peer (any client). Validate every input.
- **Trusted**: the host process and the configured `Authenticator`.

Common attack surfaces and mitigations:

| Surface                | Mitigation                                                          |
| ---------------------- | ------------------------------------------------------------------- |
| Path injection in `input` frames | Server validates each path against `operator_inputs` declarations |
| Frame size DoS         | Server enforces a configurable max frame size (default 64 KiB)      |
| Input flooding         | Server enforces per-connection input rate limit (default 60/sec)    |
| Token leakage in logs  | Tokens are never logged; only the derived role/subject is           |
| TLS                    | Optional `tls` feature uses `axum-server` + `rustls`. For production, prefer a reverse proxy (nginx, Caddy, ALB). |
| Token rotation         | Runtime-side per LSDP/1 §8 — server requires no special handling, just an authenticator that accepts rotated tokens. |
