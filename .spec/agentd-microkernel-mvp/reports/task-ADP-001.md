# Task ADP-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ADP-001 — Supervised external process lifecycle + private IPC
- **Status:** DONE
- **Commits:** `8cb7b9d` — `feat(process-supervisor): supervised process lifecycle with private socketpair IPC [ADP-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `spawn.rs`: `SpawnSpec` + `spawn()` — `AF_UNIX SOCK_STREAM` socketpair with
  the child end pinned to fd 0 (`#![forbid(unsafe_code)]` rules out
  `pre_exec`/`dup2`; the spec's "(e.g. 3)" fd number is illustrative — fd 0
  is equally fixed and keeps stdout/stderr independent pipes). Minimal env
  allowlist via `env_clear` + `envs`; child is its own process-group leader.
  `handshake()`: `AdapterBootstrap` -> `AdapterHello` over u32-BE
  length-prefixed prost frames capped at `adapters.max_frame_bytes` (4 MiB),
  pinning instance id, adapter identity, bundle digest, protocol version —
  any mismatch fails closed. `terminate()`: `AdapterShutdown` frame, grace
  deadline, then `SIGTERM`/`SIGKILL` to the child's process group
  (rustix `kill_process_group` — no unsafe in-crate) so descendants die with
  it; `wait()` normalizes `ExitReason::{Exited, Signaled, Undetermined}`.
- `child.rs`: durable `adapter_instances` lifecycle inside `KernelTxn` —
  `record_spawn` (starting, with pid + `/proc/pid/stat` start identity),
  `mark_ready`, `mark_exited`, `mark_failed`, `heartbeat`; rows fenced by
  `daemon_instance_id` so a restart inserts a fresh instance rather than
  resurrecting a stale session.
- `kernel-store`: `AdapterRead::get_instance` added (sqlite + mock).

## Evidence

`cargo test -p process-supervisor` — 7 tests over real `/bin/sh` children:
private IPC carries bytes both ways; stdout/stderr never alias the IPC fd;
no discoverable listener exists for an unrelated process to race; child
crash captured as `Exited(42)`; `sleep`-descendant killed with the group;
handshake verifies identity/digest and rejects an instance mismatch; the
instance row lifecycle persists starting -> exited with reason.
