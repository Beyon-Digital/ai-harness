# Task API-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** API-001 — Local gRPC server over Unix-domain socket
- **Status:** DONE
- **Commits:** `8022973` — `feat(control-api): local gRPC server over a locked, owner-only UDS [API-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `uds.rs`: `ControlSocket::bind(runtime_dir, &DaemonLock)` — creates the
  runtime dir at 0700, refuses to unlink a live endpoint (probes with a
  connect before removing a stale file), binds `control.sock` at 0600, and
  unlinks on drop. Binding requires proof of the daemon singleton lock, so
  a second daemon cannot steal the socket.
- `server.rs`: tonic `MvpControlApi` over UDS via `serve_with_incoming`;
  `UdsConn` wraps each `tokio::net::UnixStream` with `SO_PEERCRED` and
  implements `Connected` so creds land in request extensions.
  `PeerPrincipalMap`/`UidPrincipalMap` resolve uid -> principal; every
  handler resolves a `LocalActor` bootstrap context and rejects a
  `principal_id` claim that disagrees with the peer's mapped principal.
  Handlers hold no `KernelStore` — writes go through `CommandCoordinator`;
  read RPCs return `Unimplemented` pending API-002's narrow query port.
- `agentd::api::serve_control_api`: lock + socket + service + drain
  shutdown wired at the composition-root seam.
- `control-api/build.rs`: compiles `mvp_control.proto` with
  `extern_path(".agentos.spec.v1", "::domain::generated::contract")` —
  service stubs only, single message source of truth.

## Evidence

`cargo test -p control-api` — 7 tests: socket at 0600 (dir 0700); second
daemon bind refused (`FailedPrecondition`) while a live endpoint holds the
socket; stale dead socket replaced; graceful shutdown stops the server and
unlinks; real gRPC client smoke test (`Health` + `SubmitCommand` -> Echo
handler -> `ok` outcome); claimed-principal mismatch `PermissionDenied`;
unmapped uid `PermissionDenied`.
