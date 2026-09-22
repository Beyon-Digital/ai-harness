# Task ADP-002 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ADP-002 — Framed adapter protocol + identity-pinned handshake
- **Status:** DONE
- **Commits:** `bbc141e` — `feat(adapter-protocol): framed request/response/cancel protocol + handshake identity [ADP-002]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `framing.rs`: u32-BE length-prefixed `AdapterFrame` encode/decode;
  oversized length prefix rejected with `ResourceExhausted` before
  allocation; malformed bodies rejected with `InvalidArgument`; clean EOF
  is `None`. Cap = `adapters.max_frame_bytes` (4 MiB).
- `handshake.rs`: `ExpectedIdentity` (daemon instance + fencing epoch,
  adapter instance id, adapter id/version, expected bundle digest,
  protocol version). `bootstrap_frame(nonce)`; `verify_hello` checks the
  child's self-assertion against the supervisor's expected identity —
  wrong digest/id/version/instance/protocol all fail closed. Hello has no
  nonce field in the proto; freshness is established by the private
  socketpair channel (doc'd). `SessionPhase{AwaitingHello,Ready,Draining}`
  + `accept_inbound` reject unexpected-order frames per direction.
- `session.rs`: `dispatch_call` — writes `PortCallRequest`, loops
  `read_frame` with `set_read_timeout(remaining)`; deadline expiry sends
  `Cancel` then fails `Unavailable` (no DeadlineExceeded code in enum);
  Ping auto-answers Pong mid-call; matching `Cancel` -> `Cancelled`.
- `process-supervisor/spawn.rs` refactored onto the shared crate: the
  supervisor pins `adapter_id` + `adapter_version` on `Child` so the Hello
  is verified against the *registered* identity — AC met: a child's
  self-assertion cannot override supervisor-expected identity.

## Evidence

`cargo test -p adapter-protocol` (9 tests): wrong digest/instance/
protocol handshake fails, oversized frame rejected, unexpected-order
frames rejected, request->response roundtrip, cancel delivered,
deadline closes request, ping/pong answered during call, encode
roundtrip + clean EOF. `process-supervisor` tests re-run green against
the shared framing.
