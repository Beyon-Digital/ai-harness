# Task PST-003A — SQLite transactions and core repositories

- **Status:** DONE_WITH_CONCERNS
- **Owner:** agent-pst003a
- **Commit:** `712a2e2` — `feat(store): sqlite transactions and core repos [PST-003A]`
- **Date:** 2026-09-12
- **Depends on:** PST-001, PST-002 (satisfied)
- **Branch:** `feat/agentd-microkernel-mvp`

## Implementation notes

Files (lease-scoped, nothing else changed by this task):

| File | Role |
|---|---|
| `src/txn.rs` | `SqliteWriteTxn` / `SqliteReadTxn` guards, `KernelTxn` / `KernelReadTxn` impls, `KernelStore` impl (`begin_write`, `begin_read`, `acquire_daemon_fence`, `current_fence`) |
| `src/mapping.rs` | sqlx→`KernelError` classification, row decode helpers, unit tests for the design error table |
| `src/repos/mod.rs` | module wiring, `SharedConn`/`ConnGuard` connection sharing, `UnavailableRepo` shim for later groups |
| `src/repos/runs.rs` | `RunRead`/`RunRepo`: insert, get, list-by-task, CAS update |
| `src/repos/tasks.rs` | `TaskRead`/`TaskRepo`: insert, get, list-by-session |
| `src/repos/sessions.rs` | `SessionRead`/`SessionRepo`: insert, get |
| `src/repos/graph.rs` | `GraphRead`/`GraphRepo`: heads, dependency insert, recursive reachability |
| `src/repos/environments.rs` | `EnvironmentRead`/`EnvironmentRepo`: immutable environments and bindings |
| `src/lib.rs` | declares `mapping`, `repos`, `txn`; adds `pub(crate) SqliteKernelStore::pool` |
| `tests/cas.rs` | CAS, graph head/dependency CAS, constraint mapping, unknown-enum fail-closed |
| `tests/rollback.rs` | admission rejection, faulted write, drop rollback, explicit rollback, empty commit, single-connection reuse |
| `tests/immutability.rs` | trigger rejection for environments/bindings, duplicate conflicts, FK mapping |
| `tests/contention.rs` | barrier-raced single-winner CAS, 1 ms busy timeout mapped to `Unavailable` |

### Transaction design

- `begin_write` calls `SqlitePool::begin_with("BEGIN IMMEDIATE")` — the explicit
  write-lock statement from D2 — on its own pooled connection, then reads
  `daemon_fence.fencing_epoch` once and compares it to `ctx.daemon_epoch`
  (D6). Absent or mismatched epoch returns `FailedPrecondition`/`Never`; the
  dropped guard queues the rollback, so a rejected admission mutates nothing.
- `commit` executes `COMMIT`, `rollback` executes `ROLLBACK`. Dropping an
  unfinished guard lets the sqlx transaction guard queue a `ROLLBACK` on the
  owned connection (ignoring its result), which
  `dropped_write_returns_a_clean_single_pooled_connection` proves by reusing a
  one-connection pool immediately.
- `begin_read` acquires a plain pooled connection and issues no write
  statements.
- `sqlx::SqliteConnection` is `Send` but not `Sync`, while `KernelTxn:
  Send + Sync`. Each transaction therefore owns one
  `Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, Sqlite>>>>`
  shared by its repository views; `commit`/`rollback` take the transaction out
  of the mutex. This is internal layout only; the port is untouched.

### Mapping (design Error handling, D5)

- Extended codes are masked to their primary code: `19` splits into
  unique/PK → `Conflict`/`Never` and CHECK/FK/NOT NULL/trigger/rowid →
  `FailedPrecondition`/`Never`; `5`/`6` and their extended forms → `Unavailable`/
  `Safe`; `IOERR`/`CANTOPEN` → `Unavailable`/`Safe`; anything else →
  `Internal`/`Never`.
- Pool closed/timed out and I/O → `Unavailable`/`Safe`; row-not-found →
  `NotFound`/`Never`; unknown enum/state/id/negative counter/decode failure →
  `Internal`/`Never`.
- Every error keeps the sqlx source attached, but `KernelError::Display` never
  renders it, so constraint messages with row data do not leak.

### Repositories

- CAS updates are one parameterized `UPDATE ... WHERE key = ? AND expected
  columns` with `COALESCE(?n, column)` per patch field; `rows_affected() == 1`
  is returned as `bool` (R3.2). `run_revision` is bumped with
  `run_revision + ?`.
- `GraphRepo::insert_dependency` validates the head and expected revision and
  advances the head inside the already-immediate transaction; absent head →
  `NotFound`, revision mismatch or duplicate edge → `Conflict` (R3.3).
- `EnvironmentRepo` has no update/delete operations; trigger aborts are
  asserted in `immutability.rs` (R3.6).
- `DependencyCondition` is a wire-mirror enum in the port but the DDL column
  is the `CHECK`-constrained TEXT literal set, so `graph.rs` bridges through
  exact literals (`completed_successfully`, `any_terminal`,
  `completed_or_cancelled`) and fails closed on anything else.

### Fence methods

`KernelStore` cannot be implemented without `acquire_daemon_fence` and
`current_fence`, and PST-003A's own tests must seed a fence to open write
transactions. `txn.rs` therefore implements both with the testkit mock's
observable semantics: `BEGIN IMMEDIATE`, epoch strictly `+1`, instance
recorded, lease `i64::MAX`, read-back through `current_fence`. PST-004 owns
`fence.rs` and lease persistence and is expected to move/refine this code.

## Acceptance criteria

| Criterion | Evidence | Command |
|---|---|---|
| R3.1 persisted enums decode through domain types and fail closed | `mapping::tests::unknown_persisted_enums_fail_closed`, `cas::unknown_persisted_enum_fails_closed` (99 written with `ignore_check_constraints` → `Internal`/`Never`) | `cargo test -p kernel-store-sqlite` |
| R3.2 / P2 CAS conflicts are conflict reports; race produces exactly one winner | `cas::stale_expectation_returns_false_and_mutates_nothing`, `contention::exactly_one_writer_wins_a_raced_cas` (6 writers, one barrier, exactly 1 win, revision 1) | same |
| R3.3 writer ordering reserved with `BEGIN IMMEDIATE` for head and claim mutations | `txn.rs` `begin_with("BEGIN IMMEDIATE")` on every write; `cas::graph_head_cas_and_dependency_insertion`, `cas::claim_patch_is_applied_with_the_cas` | same |
| R3.4 constraint and busy failures map to the stable codes in the design table | `mapping::tests::{unique_and_primary_key_codes_are_conflicts, constraint_codes_are_failed_preconditions, busy_and_locked_codes_are_retry_safe_unavailable}`, `cas::unique_violation_is_conflict_and_constraint_is_failed_precondition`, `contention::contended_begin_write_maps_busy_to_unavailable` | same |
| R3.5 / P1 failed or dropped transactions leave zero partial rows | `rollback::fault_after_successful_write_leaves_zero_rows`, `rollback::drop_without_commit_rolls_back`, `rollback::explicit_rollback_discards_writes`, `rollback::dropped_write_returns_a_clean_single_pooled_connection` | same |
| R3.6 immutable records rejected by the storage layer | `immutability::environment_update_and_delete_are_rejected_by_triggers` (extended code 1811, `immutable record`), duplicate/FK cases | same |
| N2 concurrency tests use barriers/fault points, no sleeps | `tokio::sync::Barrier` in `contention.rs`, `testkit::faults::ArmedFaults` in `rollback.rs`; no `sleep` in any test | `rg "sleep" tests/` |
| No file outside `files:` changed | commit contains exactly the 13 leased paths | `git show --stat 712a2e2` |

All 32 crate tests pass: 6 unit + 8 bootstrap (PST-001) + 6 cas + 2
contention + 4 immutability + 6 rollback.

## RED / GREEN

RED — `cargo test -p kernel-store-sqlite --test cas --test rollback` before
implementation (both targets fail to compile against the missing store impl):

```
error[E0599]: no method named `acquire_daemon_fence` found for struct `SqliteKernelStore` in the current scope
error[E0282]: type annotations needed
error[E0599]: no method named `begin_read` found for reference `&SqliteKernelStore` in the current scope
error[E0282]: type annotations needed
error[E0599]: no method named `begin_write` found for struct `SqliteKernelStore` in the current scope
...
error: could not compile `kernel-store-sqlite` (test "cas") due to 61 previous errors; 1 warning emitted
```

The `rollback` target failed the same way (`acquire_daemon_fence`, `begin_write`,
and `begin_read` unresolved), so both newly written suites were RED before the
implementation landed.

GREEN — `cargo test -p kernel-store-sqlite`:

```
running 6 tests   (unittests src/lib.rs)
test mapping::tests::busy_and_locked_codes_are_retry_safe_unavailable ... ok
test mapping::tests::constraint_codes_are_failed_preconditions ... ok
test mapping::tests::unique_and_primary_key_codes_are_conflicts ... ok
test mapping::tests::malformed_identifiers_and_negative_counters_fail_closed ... ok
test mapping::tests::unknown_codes_are_internal ... ok
test mapping::tests::unknown_persisted_enums_fail_closed ... ok
test result: ok. 6 passed; 0 failed

running 8 tests   (tests/bootstrap.rs)
test result: ok. 8 passed; 0 failed

running 6 tests   (tests/cas.rs)
test matching_cas_applies_patch_and_bumps_revision ... ok
test graph_head_cas_and_dependency_insertion ... ok
test claim_patch_is_applied_with_the_cas ... ok
test unknown_persisted_enum_fails_closed ... ok
test stale_expectation_returns_false_and_mutates_nothing ... ok
test unique_violation_is_conflict_and_constraint_is_failed_precondition ... ok
test result: ok. 6 passed; 0 failed

running 2 tests   (tests/contention.rs)
test contended_begin_write_maps_busy_to_unavailable ... ok
test exactly_one_writer_wins_a_raced_cas ... ok
test result: ok. 2 passed; 0 failed

running 4 tests   (tests/immutability.rs)
test duplicate_environment_for_run_is_conflict ... ok
test binding_without_environment_is_failed_precondition ... ok
test environment_update_and_delete_are_rejected_by_triggers ... ok
test duplicate_bindings_are_conflict ... ok
test result: ok. 4 passed; 0 failed

running 6 tests   (tests/rollback.rs)
test dropped_write_returns_a_clean_single_pooled_connection ... ok
test explicit_rollback_discards_writes ... ok
test drop_without_commit_rolls_back ... ok
test missing_or_stale_epoch_rejects_transaction ... ok
test fault_after_successful_write_leaves_zero_rows ... ok
test empty_commit_is_a_noop_success ... ok
test result: ok. 6 passed; 0 failed
```

Workspace gates from `agent-os/`:

- `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean (whole workspace).
- The contention suite was re-run three times; all runs green, no flakes.

## Concerns

1. **`UnavailableRepo` coordination seam.** `KernelTxn`/`KernelReadTxn` are
   single object-safe traits that require every repository accessor, but
   `effects`, `resources`, `timers`, `security`, `config`, `workspaces`,
   `adapters`, `artifacts`, `loop_turns`, `idempotency`, and `streams` files
   belong to PST-003B/PST-005. `repos/mod.rs` therefore ships a generated
   `UnavailableRepo` whose operations fail closed with
   `FailedPrecondition`/`Never`; `txn.rs` wires those accessors to it. PST-003B
   and PST-005 should add their modules and replace the corresponding
   accessors, deleting the shim. No fabricated rows are ever returned.
2. **Fence methods live in `txn.rs` for now.** Implemented with mock-matching
   semantics so transactions are testable before PST-004; the lease is
   `i64::MAX` and renewal/draining are out of scope. PST-004 owns `fence.rs`
   and should move/replace this implementation; its task text says "fence
   claim, read, assert, lease persistence", so no design change is implied.
3. **`DependencyCondition` model mismatch.** The port type is a wire-mirror
   enum; the DDL stores TEXT `CHECK` literals. `graph.rs` bridges explicitly
   and fails closed. Using `Unspecified` produces a CHECK abort →
   `FailedPrecondition` (never persisted). A future port revision may prefer a
   state-string type, but the port is frozen for this module.
4. **`updated_at_ms` is not advanced by `cas_update`.** `RunPatch` carries no
   timestamp and the testkit mock also leaves `updated_at_ms` at its insert
   value, so SQLite keeps parity with the mock. If the kernel wants
   clock-driven timestamps on CAS, the port must supply the value.
5. **`begin_read` is not a snapshot transaction.** It uses a plain pooled
   connection per the task context ("plain read connection with no write
   statements"), so a read transaction sees per-statement committed state.
   Callers needing a multi-statement snapshot should use a write transaction
   or a future read-transaction variant.
6. **Trigger mapping is only unit-covered indirectly.** There is no port
   operation that updates an immutable table (by design), so the trigger-abort
   → `FailedPrecondition` classification is unit-tested at the code level
   (`classify_sqlite(1811)`) and the trigger behaviour is integration-tested
   through a direct connection asserting SQLite extended code 1811.

## Post-review fix (test quality)

- **Commit:** `3c3a359` — `test(store): cover multi-write failure rollback and absent fence [PST-003A]`
- **Scope:** `agent-os/crates/kernel-store-sqlite/tests/rollback.rs` only; no production changes.

### Finding 1 — the armed testkit fault never reached the store

`fault_after_successful_write_leaves_zero_rows` armed and triggered
`ArmedFaults`, but no production code consults a `FaultInjector`, so the test
was observationally identical to `drop_without_commit_rolls_back`.

Replaced with `constraint_failure_after_two_writes_rolls_back_everything`,
which manufactures the abort through a real store operation:

1. begin a write transaction;
2. `sessions().insert` (success) and `tasks().insert` (success) — two
   statements against the immediate transaction;
3. re-insert the same session → `Conflict`/`Never`;
4. drop the guard;
5. assert both successful writes are absent through a fresh read transaction;
6. begin, commit, and re-read on the same store to prove the pool and writer
   lock are reusable after the failed transaction.

The now-unused `domain::faults::FaultInjector` and `testkit::faults::ArmedFaults`
imports were removed. (`ArmedFaults` remains exercised by `testkit`'s own
suite; PST-003A no longer misuses it as a stand-in for an integration seam.)

### Finding 2 — the missing-fence branch was never exercised

`missing_or_stale_epoch_rejects_transaction` only covers mismatches against an
existing epoch-1 fence. Added `absent_fence_rejects_write_transaction`: a
fresh store with **no** `acquire_daemon_fence` call rejects `begin_write` with
`FailedPrecondition`/`Never` for both a `0` and a `1` context epoch, the
message names the fence as `absent`, and a subsequent
`acquire_daemon_fence` (epoch 1) makes writes possible.

### Fix evidence (from `agent-os/`)

```
cargo test -p kernel-store-sqlite --test rollback
running 7 tests
test dropped_write_returns_a_clean_single_pooled_connection ... ok
test constraint_failure_after_two_writes_rolls_back_everything ... ok
test absent_fence_rejects_write_transaction ... ok
test drop_without_commit_rolls_back ... ok
test empty_commit_is_a_noop_success ... ok
test explicit_rollback_discards_writes ... ok
test missing_or_stale_epoch_rejects_transaction ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.37s
```

- `cargo test -p kernel-store-sqlite` — 33 passed, 0 failed (6 unit + 8
  bootstrap + 6 cas + 2 contention + 4 immutability + 7 rollback).
- `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.

Task remains in review; not marked done.
