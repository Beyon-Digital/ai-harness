# Task FND-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** FND-001 — Bootstrap the Rust workspace, crates, and quality gates
- **Agent:** agent-fnd001
- **Status:** DONE
- **Commit:** `52a09e3` — `chore(workspace): bootstrap crates and quality gates [FND-001]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What was created

A compilable 31-crate Rust workspace under `agent-os/`:

- `Cargo.toml` — all 31 members, `[workspace.package]` policy, and `[workspace.dependencies]`
  for tokio, tracing, tracing-subscriber, serde, serde_json, serde_yaml, prost, prost-build,
  tonic, uuid (v7, serde), sha2, sqlx (runtime-tokio, sqlite, macros), thiserror, async-trait,
  tempfile, proptest, protoc-bin-vendored, anyhow, plus path entries for every crate.
- `Cargo.lock` — committed resolution (139 packages including dev/build dependencies).
- `rust-toolchain.toml` — pins channel `1.94.0` with `rustfmt` and `clippy`.
- `.cargo/config.toml` — aliases `check-all`, `lint`, `test-all` wrapping the quality commands.
- `.github/workflows/ci.yml` — macOS quality job (fmt, clippy `-D warnings`, test, check,
  N2 hygiene grep) plus repository/build-pack validator job at repo root.
- 31 `crates/<name>/Cargo.toml` manifests and crate roots: foundation crates (`errors`,
  `domain`, `testkit`, `observability`, `agentd`) declare exactly the dependencies the task
  specifies; the other 26 declare their crate-map dependencies and contain only a crate doc
  comment plus `#![forbid(unsafe_code)]`.
- Pre-declared module stubs (module doc comment only): `domain::{ids, provider, time, faults,
  run, effect, security, resource, generated}`, `errors::codes`, `testkit::{clock, ids, faults,
  process}`, `observability::classification`, and `agentd::{lock, recovery, api, workers::{mod,
  outbox, scheduler, loops}}`. `agentd/src/main.rs` declares its modules and returns
  `ExitCode::SUCCESS` with no DB, sockets, or async runtime use.

Total: 89 files, matching the task `files:` list exactly (verified with a script; 0 missing,
0 extra under `agent-os/` excluding `target/`).

## Corrections to the interrupted partial work

The prior agent's work was structurally complete but failed two gates; I fixed both:

1. **`cargo fmt --check` failure** — `crates/domain/src/lib.rs` and `crates/testkit/src/lib.rs`
   declared modules in a non-alphabetical order that rustfmt rejects (`reorder_modules`).
   Reordered alphabetically; semantics unchanged. The task context listed the domain modules
   in a different literal order, but the mandatory fmt gate takes precedence.
2. **Invalid `ci.yml` YAML** — the step name `Hygiene grep (N2: no wall-clock sleep in tests)`
   contained an unquoted `": "`, making the whole workflow unparseable (PyYAML failed at
   line 26). Quoted the step name; workflow now parses and both jobs/steps verified.

## Acceptance criteria

| # | Criterion | Met | Evidence |
|---|---|---|---|
| 1 | R13.1 — all four quality commands pass | **MET** | `cargo check --workspace && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → `COMBINED EXIT: 0` |
| 2 | R13.2 — `agentd` exits cleanly; no DB or socket code exists | **MET** | `cargo run -q -p agentd; echo $?` → `0`; `grep -rnEi 'sqlx\|rusqlite\|TcpListener\|UnixListener\|bind\(' crates/agentd/src` → none |
| 3 | R13.3 / P2 — 31 crates, acyclic dependency graph | **MET** | `cargo metadata --no-deps` package count → `31`; full `cargo check --workspace --locked` resolves successfully (Cargo rejects cycles) |
| 4 | N2 — CI hygiene grep present | **MET** | `.github/workflows/ci.yml` step greps for `thread::sleep\|tokio::time::sleep` in `crates/**/tests/` and `#[cfg(test)]` modules; ran the snippet locally → `hygiene grep: no violations (exit 0)`; `ci.yml` validated as parseable YAML |
| 5 | No file outside `files:` changed | **MET** | Script compared the FND-001 `files:` list against the worktree: 89/89 present, 0 extra; `git show --stat HEAD` = exactly 89 files |

Additional invariant checks:

- `#![forbid(unsafe_code)]` present in all 31 crate roots (`grep -L` returned nothing).
- No `unwrap()`/`expect()` anywhere under `crates/` (stub-only, verified by grep).
- No deferred-work markers (`TODO`/`FIXME`/`unimplemented!`/`todo!`/`TBD`) under `crates/`.
- `cargo check --workspace --locked` → exit 0 (committed lock is in sync).
- Aliases exercised: `cargo check-all` exit 0, `cargo test-all` exit 0.

## Verification output (real, condensed)

```
$ cargo check --workspace && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.87s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.39s
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.02s
   Doc-tests workspace
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
COMBINED EXIT: 0

$ cargo run -q -p agentd; echo $?
0

$ python3 -c "...cargo metadata --format-version 1 --no-deps...; print(len(m['packages']))"
31

$ cargo check --workspace --locked
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 37.92s
locked exit: 0

$ cargo fmt --check
FMT OK

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 34s
clippy exit: 0

$ git log --oneline -1
52a09e3 chore(workspace): bootstrap crates and quality gates [FND-001]
```

Toolchain used: `rustc 1.94.0 (4a4ef493e 2026-03-02)`, `cargo 1.94.0 (85eff7c80 2026-01-15)`
(selected automatically by `rust-toolchain.toml`).

## Files changed

89 files, all within the task `files:` list — see `git show --stat 52a09e3`. Highlights:
`agent-os/Cargo.toml`, `agent-os/Cargo.lock`, `agent-os/rust-toolchain.toml`,
`agent-os/.cargo/config.toml`, `agent-os/.github/workflows/ci.yml`, and the 31 crate
manifest/source pairs.

## Concerns

1. **Inferred dependency edges for the 26 non-foundation crates.** `architecture/crate-map.md`
   specifies ownership and dependency *direction* but not per-crate edges. The declared edges
   (e.g. `effects → event-journal, kernel-store`; `workspace → sandbox`; `agentctl →
   control-api, events`) are an interpretation of the map and all resolve acyclically. If later
   tasks need a different edge, their own manifests tasks should be re-leased.
2. **Module declaration order.** The task text lists `domain` modules in a non-rustfmt order;
   the committed order is alphabetical to satisfy `cargo fmt --check`. No semantic difference.
3. **tokio features.** The workspace dep uses `macros`, `rt-multi-thread`, `sync`, `time`
   beyond the bare crate name listed in the task, so later composition-root tasks do not need
   to edit the shared workspace manifest.
4. **`domain/src/generated.rs` is intentionally an empty stub.** FND-002 owns filling it with
   the prost include/re-export; it is pre-declared here so FND-002 never edits `domain/src/lib.rs`.

---

## Review fix (appended after re-review request)

**Finding addressed (Important):** `crates/domain/Cargo.toml` had no `[dev-dependencies]`, but
FND-002's `crates/domain/tests/contract_codegen.rs` (regenerates protos into temp dirs) and
FND-003's `crates/domain/tests/prop_enums.rs` need test-only dependencies without owning the
manifest.

**What changed:** added a minimal `[dev-dependencies]` section using workspace declarations:

```toml
[dev-dependencies]
prost-build.workspace = true
proptest.workspace = true
protoc-bin-vendored.workspace = true
tempfile.workspace = true
```

Commit: `57ce32b` — `fix(domain): declare test dev-dependencies [FND-001]` (2 files:
`agent-os/crates/domain/Cargo.toml`, `agent-os/Cargo.lock`).

**Covering commands and real output:**

```
$ cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.33s
check exit: 0

$ cargo test --workspace
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit: 0

$ cargo check --workspace --locked
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.77s
locked exit: 0
```

**Lock resolution confirmed:**

```
domain lock deps: ['proptest', 'prost', 'prost-build', 'protoc-bin-vendored', 'serde', 'tempfile', 'uuid']
prost-build -> True
protoc-bin-vendored -> True
tempfile -> True
proptest -> True
```

No other files changed. Task remains in review; awaiting controller re-review.
