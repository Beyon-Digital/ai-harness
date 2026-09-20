---
name: testing-agent-os
description: How to test the agent-os Rust microkernel workspace in ai-harness — build/test commands, why agentd cannot be driven over a transport, and the CommandCoordinator envelope-dispatch pattern used as the end-to-end path.
---

# Testing agent-os (ai-harness)

Rust workspace lives at `agent-os/` under the repo root (not the root itself).
Toolchain is pinned by `agent-os/rust-toolchain.toml` (1.94.0); run all cargo
commands from `agent-os/`.

## Commands

- Build/boot the daemon stub: `cargo run -p agentd` (compiles, exits 0).
- Workspace tests: `cargo test --workspace` (~430 tests across ~100 binaries, ~1-2 min warm).
- Lint/fmt gates: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`.
- Repo validators: `python3 tools/validate_repo.py`,
  `bash agent-os/scripts/check-contract-mirror.sh`,
  `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` — all from repo root.

## No transport exists — use CommandCoordinator for end-to-end

`crates/agentd/src/main.rs` is a placeholder that exits immediately; `control-api`
and `agentctl` are stubs. There is NO socket/HTTP/gRPC path to submit commands.
The realistic end-to-end is driving a `command_coordinator::envelope::CommandEnvelope`
through `CommandCoordinator::execute` inside a test — the same dispatch path
(registry name lookup like `agentos.spec.v1.ResolveUnknownEffect` -> handler ->
sqlite txn) a future daemon will use.

## Harness pattern (copy from runtime/tests/effect_recovery.rs)

- `SqliteKernelStore::open(StoreConfig{path: <tempdir>/kernel.db, ...})` for a real sqlite store.
- `store.acquire_daemon_fence(DaemonInstanceId::new(ids))` -> `FixedFence(epoch)` for the coordinator.
- `runtime::register_handlers(&mut CommandRegistry::new(), runtime::RuntimeDeps{clock, ids})`.
- Determinism: `testkit::ids::DeterministicIds`, `testkit::clock::TestClock`,
  `testkit::faults::ArmedFaults`.
- Prost payloads: `domain::generated::contract::*` messages encoded via `prost::Message::encode_to_vec`.
- Add scratch integration tests as new files under `crates/<crate>/tests/`; cargo picks
  them up automatically. Delete before finishing.

## Devin Secrets Needed

None for kernel/library testing. `OPEN_ROUTER` exists in the session env for
ai/llm features but no crate consumes it yet.
