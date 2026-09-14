# Task SEC-000 — Declare security-module dependencies

- Status: complete (awaiting review)
- Agent: agent-sec000
- Spec: agentd-security
- Commit: `21f5c0b` — chore(workspace): declare security module dependencies [SEC-000]
- Branch: feat/agentd-microkernel-mvp

## What changed

Manifest-only change. The root workspace gained the two new dependency
declarations; every per-crate entry is appended without removing or reordering
existing lines. All per-crate additions resolve via `.workspace = true`.

| File | Change |
| --- | --- |
| `agent-os/Cargo.toml` | `[workspace.dependencies]` + `zeroize = "1"`, + `security-framework = "2"` |
| `agent-os/crates/identity/Cargo.toml` | append `async-trait` (`domain`, `errors`, `kernel-store` already present); new `[dev-dependencies]` `kernel-store-sqlite`, `testkit`, `tempfile`, `tokio`, `proptest` |
| `agent-os/crates/permissions/Cargo.toml` | append `async-trait` (`domain`, `errors`, `identity` already present); new `[dev-dependencies]` `proptest` |
| `agent-os/crates/approvals/Cargo.toml` | append `kernel-store`, `command-coordinator`, `events`, `sha2`, `async-trait`, `prost`; new `[dev-dependencies]` `kernel-store-sqlite`, `testkit`, `tempfile`, `tokio`, `proptest` |
| `agent-os/crates/secrets/Cargo.toml` | append `identity`, `permissions`, `observability`, `async-trait`, `zeroize`, `sha2`; new `[target.'cfg(target_os = "macos")'.dependencies]` `security-framework`; new `[dev-dependencies]` `testkit`, `tokio` |
| `agent-os/Cargo.lock` | resolved by `cargo check --workspace`; +67 lines, 4 new packages |

`tokio` features: the single workspace `tokio` entry already carries `macros`,
`rt-multi-thread`, `sync`, `time`; `.workspace = true` inherits that union for
both `[dependencies]` and `[dev-dependencies]`, satisfying the task's
"macros, rt-multi-thread" dev requirement and matching the convention in
`runtime`, `kernel-store-sqlite`, and `run-graph`.

## Acceptance criteria

### 1. All crates resolve; lock updated — MET

```
== cargo check --workspace ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5m 51s
exit: 0

Locking 4 packages to latest Rust 1.94 compatible versions
  Adding core-foundation v0.9.4
  Adding core-foundation-sys v0.8.7
  Adding security-framework v2.11.1 (available: v3.7.0)
  Adding security-framework-sys v2.17.0

== cargo check --workspace --locked (committed lockfile) ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.78s
exit: 0
```

Lockfile delta: 4 new packages above; `zeroize v1.9.0` was already present in
the lock as a transitive dependency and is now also lifted into the workspace
graph for `secrets`. New crate edges appearing in the lock: `async-trait`,
`command-coordinator`, `events`, `identity`, `kernel-store`,
`kernel-store-sqlite`, `observability`, `permissions`, `proptest`, `prost`,
`sha2`, `tempfile`, `testkit`, `tokio`, `zeroize` (plus the four
security-framework packages and their `bitflags`/`libc` transitives).

### 2. N3/G1/G2 — no contract or schema change; gates and validators pass — MET

```
== cargo test --workspace --no-run ==
  Executable tests/store_mock.rs (target/debug/deps/store_mock-2150b0410600352e)
  Executable unittests src/lib.rs (target/debug/deps/workspace-561d4c6a0964266a)
exit: 0

== cargo clippy --workspace --all-targets -- -D warnings ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.09s
exit: 0 (no warnings)

== cargo fmt --check ==
exit: 0

== python3 tools/validate_repo.py ==
OK: 290 markdown, 13 canonical ports, no link/schema/catalog errors
exit: 0

== python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py ==
BUILD PACK OK: 59 tasks, 111 markdown files, contracts locked
exit: 0

== bash agent-os/scripts/check-contract-mirror.sh ==
./protocols/external_adapter.proto: OK
contract mirror check OK
exit: 0
```

N3 is satisfied trivially: only manifests and the lock are touched; no
`contracts/`, `schema/`, or build-pack file appears in the commit.

### 3. No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' HEAD
21f5c0b chore(workspace): declare security module dependencies [SEC-000]

 agent-os/Cargo.lock                    | 67 ++++++++++++++++++++++++++++++++++
 agent-os/Cargo.toml                    |  2 +
 agent-os/crates/approvals/Cargo.toml   | 13 +++++++
 agent-os/crates/identity/Cargo.toml    |  8 ++++
 agent-os/crates/permissions/Cargo.toml |  4 ++
 agent-os/crates/secrets/Cargo.toml     | 13 +++++++
 6 files changed, 107 insertions(+)
```

## Verification checklist

- [x] From `agent-os/`: `cargo check --workspace` — exit 0
- [x] From `agent-os/`: `cargo test --workspace --no-run` — exit 0
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0
- [x] `cargo fmt --check` — exit 0
- [x] `cargo check --workspace --locked` — committed lockfile in sync, exit 0
- [x] Repo validators and contract mirror check — all exit 0
- [x] Commit retried on `index.lock` after two seconds; no lock contention occurred (succeeded on first true attempt)
- [x] No deferred-work markers introduced

## Concerns

- `.spec/agentd-security/{ledger,tasks}.md` carry spec-flow bookkeeping edits
  from claim/start/review. They were already modified before this task began,
  are outside the lease, and were intentionally left uncommitted; they are not
  part of `21f5c0b`.
- The untracked `agent-os/crates/run-graph/tests/graph.proptest-regressions`
  pre-existed this task (another agent's artifact) and was left untouched.
- `security-framework = "2"` resolves to 2.11.1 on this machine; 3.7.0 exists
  upstream. The task pins `"2"`, so the older major is intentional and applies
  only to the macOS target gate.
- `zeroize` was not newly added to `Cargo.lock` (already resolved transitively
  at 1.9.0); the workspace declaration is what changes, so the lock delta is
  only the four security-framework packages.
