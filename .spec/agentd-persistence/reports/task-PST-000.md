# Task PST-000 — Declare persistence-module dependencies

- Status: complete (awaiting review)
- Agent: agent-pst000
- Spec: agentd-persistence
- Commit: `8434bd0` — chore(workspace): declare persistence module dependencies [PST-000]
- Branch: feat/agentd-microkernel-mvp

## What changed

Prior interrupted work was verified against the task text; nothing required by the
task was missing. The commit contains only the six leased paths:

| File | Change |
| --- | --- |
| `agent-os/crates/kernel-store/Cargo.toml` | append `async-trait.workspace = true` |
| `agent-os/crates/kernel-store-sqlite/Cargo.toml` | append deps `sqlx`, `tokio`, `async-trait`, `observability`; add `[dev-dependencies]` `testkit`, `tempfile`, `proptest`, `tokio` (`kernel-store`, `domain`, `errors` were already present) |
| `agent-os/crates/testkit/Cargo.toml` | append deps `kernel-store`, `errors`, `async-trait`; append dev-dep `tokio` (`proptest` was already present) |
| `agent-os/crates/identity/Cargo.toml` | append `kernel-store.workspace = true` (`domain`, `errors` already present) |
| `agent-os/crates/events/Cargo.toml` | append `kernel-store.workspace = true` (`domain`, `errors` already present) |
| `agent-os/Cargo.lock` | regenerated (sqlx and its transitive graph added; 1385 insertions / 47 deletions) |

All entries are append-only; no existing dependency lines were removed or reordered.
Every referenced dependency resolves from existing `[workspace.dependencies]` entries
(`async-trait`, `sqlx`, `tokio`, `tempfile`, `proptest`, `testkit`, `kernel-store`,
`observability`); the root manifest was not edited. Feature requirements are inherited
from the workspace definitions: `sqlx` (`runtime-tokio`, `sqlite`), `tokio`
(`macros`, `rt-multi-thread` — transitively enables `rt`).

## Acceptance criteria

### 1. Each crate resolves the dependencies its tasks need; `Cargo.lock` updated — MET

`cargo check --workspace --locked` passes, proving the committed lockfile is fully in
sync with all manifests:

```
== cargo check --workspace --locked ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.75s
exit: 0
```

Lockfile entry for the crate with the largest dependency delta:

```
name = "kernel-store-sqlite"
version = "0.1.0"
dependencies = [
 "async-trait",
 "domain",
 "errors",
 "kernel-store",
 "observability",
 "proptest",
 "sqlx",
 "tempfile",
 "testkit",
 "tokio",
]
```

### 2. G1 — full workspace check, tests, clippy, and fmt still pass — MET

```
== cargo check --workspace ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.58s
exit: 0

== cargo test --workspace --no-run ==
  Executable tests/prop_faults.rs (target/debug/deps/prop_faults-1baa843fe4e2f210)
  Executable unittests src/lib.rs (target/debug/deps/workspace-561d4c6a0964266a)
exit: 0

== cargo clippy --workspace --all-targets -- -D warnings ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.22s
exit: 0

== cargo fmt --check ==
exit: 0
```

Full resolution runs before the cached re-runs above also passed
(`cargo check --workspace` finished in 5m 43s; `cargo test --workspace --no-run`
built every test binary; `cargo clippy` finished in 1m 07s).

### 3. N3 — no contract, schema, or build-pack file changed — MET

`git show --stat HEAD` touches only the five manifests and `agent-os/Cargo.lock`.
Both pack validators and the contract-mirror check stay green:

```
BUILD PACK OK: 59 tasks, 111 markdown files, contracts locked          (exit 0)
OK: 249 markdown, 13 canonical ports, no link/schema/catalog errors    (exit 0)
contract mirror check OK                                               (exit 0)
```

### 4. No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' HEAD
8434bd0 chore(workspace): declare persistence module dependencies [PST-000]

 agent-os/Cargo.lock                            | 1385 +++++++++++++++++++++++-
 agent-os/crates/events/Cargo.toml              |    1 +
 agent-os/crates/identity/Cargo.toml            |    1 +
 agent-os/crates/kernel-store-sqlite/Cargo.toml |   10 +
 agent-os/crates/kernel-store/Cargo.toml        |    1 +
 agent-os/crates/testkit/Cargo.toml             |    4 +
 6 files changed, 1355 insertions(+), 47 deletions(-)
```

No contract, schema, build-pack, or source file is in the commit.

## Verification checklist

- [x] From `agent-os/`: `cargo check --workspace && cargo test --workspace --no-run` — exit 0
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0
- [x] `cargo fmt --check` — exit 0
- [x] `cargo check --workspace --locked` — lockfile committed in sync
- [x] Pack validators and mirror check — all exit 0

## Concerns

- `.spec/agentd-persistence/{design,ledger,tasks}.md` had pre-existing uncommitted
  spec-flow bookkeeping edits in the working tree. They are outside this task's lease
  and were intentionally left uncommitted; they are not part of `8434bd0`.
- The workspace `tokio` definition does not explicitly list the `rt` feature; the
  task's `rt` requirement is satisfied transitively because `rt-multi-thread` enables
  `rt` in tokio 1.x. Likewise `sqlx` inherits `runtime-tokio` and `sqlite` (plus
  `macros`) from the workspace definition.
