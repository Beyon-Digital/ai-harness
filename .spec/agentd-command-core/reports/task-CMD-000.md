# Task CMD-000 Report

- **Spec:** agentd-command-core
- **Task:** CMD-000 — Declare command-coordinator dependencies
- **Agent:** agent-cmd000
- **Status:** DONE
- **Commit:** `74b327a` — `chore(workspace): declare command-coordinator dependencies [CMD-000]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What changed

`agent-os/crates/command-coordinator/Cargo.toml` (append-only):

- Added `async-trait.workspace = true` to `[dependencies]`.
- Added a new `[dev-dependencies]` section with `tokio.workspace = true`,
  `testkit.workspace = true`, `tempfile.workspace = true`, and
  `kernel-store-sqlite.workspace = true`.

`agent-os/Cargo.lock`: `cargo check --workspace` resolved the
`command-coordinator` package entry to additionally list `async-trait` (normal) and
`kernel-store-sqlite`, `tempfile`, `testkit`, `tokio` (dev); 5 added lines, no other
package changed.

All five dependencies were already declared in the root
`[workspace.dependencies]`, so every entry uses `.workspace = true`; existing lines
were not removed, reordered, or reformatted. The workspace `tokio` declaration
already enables `macros`, `rt-multi-thread`, `sync`, and `time`, satisfying the
required dev features. `testkit`/`kernel-store-sqlite` appear only as dev-dependencies,
so no production crate depends on testkit.

## Acceptance criteria

The task lists no numbered criteria; the steps and referenced requirements map as:

| # | Requirement / step | Met | Evidence |
|---|---|---|---|
| 1 | Manifest declares `async-trait` + dev `tokio`, `testkit`, `tempfile`, `kernel-store-sqlite` via `.workspace = true`, append-only | **MET** | `git diff` shows 7 added lines, 0 removed; see diff below |
| 2 | Resolve the lock with `cargo check --workspace` | **MET** | exit 0; `Cargo.lock` gained exactly the 5 new edges |
| 3 | `cargo test --workspace --no-run` passes | **MET** | exit 0 |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` passes | **MET** | exit 0 |
| 5 | `cargo fmt --check` passes | **MET** | exit 0, no output |
| 6 | G1 — workspace gates stay green | **MET** | all four commands exit 0 |
| 7 | N3 — no contract/schema/build-pack changes | **MET** | commit touches only `Cargo.toml` + `Cargo.lock`; no contract or schema file in `git show --stat` |
| 8 | Commit scoped with task id | **MET** | `74b327a`, 2 files, message contains `[CMD-000]` |
| 9 | No deferred-work markers | **MET** | diff contains no `TODO`/`FIXME`/`unimplemented!`/`todo!` |

G2 was not exercised by this task (no pack validators were run; no contract lock or
schema files were touched).

## Command output (real)

```
$ rustc --version && cargo --version
rustc 1.94.0 (4a4ef493e 2026-03-02)
cargo 1.94.0 (85eff7c80 2026-01-15)

$ cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.04s
check_exit=0

$ cargo test --workspace --no-run
  Executable tests/prop_faults.rs (target/debug/deps/prop_faults-1baa843fe4e2f210)
  Executable tests/store_mock.rs (target/debug/deps/store_mock-2150b0410600352e)
  Executable unittests src/lib.rs (target/debug/deps/workspace-561d4c6a0964266a)
test_exit=0

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.11s
clippy_exit=0

$ cargo fmt --check
fmt_exit=0

$ git log --oneline -1
74b327a chore(workspace): declare command-coordinator dependencies [CMD-000]
```

## Manifest diff

```diff
 kernel-store.workspace = true
+async-trait.workspace = true
+
+[dev-dependencies]
+tokio.workspace = true
+testkit.workspace = true
+tempfile.workspace = true
+kernel-store-sqlite.workspace = true
```

## Lock diff (condensed)

```diff
 name = "command-coordinator"
 dependencies = [
+ "async-trait",
  "domain",
  "errors",
  "event-journal",
  "events",
  "kernel-store",
+ "kernel-store-sqlite",
+ "tempfile",
+ "testkit",
+ "tokio",
 ]
```

## Files changed

- `agent-os/crates/command-coordinator/Cargo.toml` (leased)
- `agent-os/Cargo.lock` (leased)

`git show --stat 74b327a` = 2 files changed, 12 insertions(+), 0 deletions.

## Concerns

1. `tokio` is declared as a plain `tokio.workspace = true` dev-dependency; the
   required `macros`/`rt-multi-thread` features come from the shared workspace
   declaration (which also adds `sync`/`time`). This matches the existing
   convention in `kernel-store-sqlite`/`testkit`; if per-crate feature narrowing is
   desired later, the workspace entry would need changing (not this task).
2. `kernel-store-sqlite` and `testkit` are dev-only, as required — CMD-001A/CMD-001B
   integration tests can now construct a real SQLite store and testkit doubles
   without further manifest edits.
