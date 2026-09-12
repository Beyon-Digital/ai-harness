# Task PST-005 — Idempotency, streams, and the outbox API

- **Status:** DONE_WITH_CONCERNS
- **Owner:** agent-pst005b
- **Commit:** `949feef` — `feat(store): idempotency and transactional outbox [PST-005]`
- **Date:** 2026-09-12
- **Depends on:** PST-003B, PST-004 (satisfied)
- **Branch:** `feat/agentd-microkernel-mvp`

## Implementation notes

This task completed an aborted agent's partial work. The partial work was
inspected against the task, the `events outbox staging` interface, the Error
handling table, and D9/D10 before anything was added.

### Carried over from the partial work (verified, kept)

| File | Role |
|---|---|
| `crates/kernel-store-sqlite/src/repos/idempotency.rs` | `lookup`/`insert` over `idempotency_records`; unique `(principal_id, idempotency_key)` enforced by the storage constraint, mapped to `Conflict` by D5 |
| `crates/kernel-store-sqlite/src/repos/streams.rs` | `allocate` (single `INSERT ... ON CONFLICT DO UPDATE SET last_sequence = last_sequence + 1 RETURNING last_sequence`, D10), `insert_outbox`, `scan_unpublished` (`ORDER BY stream_key, sequence`, null journal marker), `mark_published` (publication columns only, `NotFound` when absent) |
| `crates/kernel-store-sqlite/src/repos/mod.rs` | module declarations for `idempotency`/`streams`; `UnavailableRepo` shim and both unavailable arms removed |
| `crates/kernel-store-sqlite/src/txn.rs` | write transaction now constructs `SqliteIdempotencyRepo`/`SqliteStreamRepo`; `idempotency()`/`streams()` return the real repositories |
| `crates/events/src/lib.rs` / `crates/events/src/outbox.rs` | `pub mod outbox;`, `DraftEvent`, `stage(txn, draft) -> Result<u64>` allocating then inserting inside the caller's transaction (D9) |
| `crates/kernel-store-sqlite/tests/idempotency.rs`, `tests/outbox.rs` | acceptance tests for R5/R6 (see below) |

### Changes and completions made in this task

- Removed `use domain::ids::*;` from `repos/mod.rs` (left unused by the shim
  removal; would fail `-D warnings`).
- Kept `mapping::unavailable` linked with a `const _: fn(&'static str) -> errors::KernelError`
  assertion in `repos/mod.rs`, because `mapping.rs` is outside this task's
  file lease and would otherwise be dead code (concern 2).
- Fixed `tests/outbox.rs::rollback_discards_outbox_rows_and_stream_allocations`:
  the partial fixture inserted sequence `1` directly and then expected
  `allocate()` to return `2`; the stream head is advanced only by `allocate`,
  so the test now allocates first (`RED` evidence below).
- Rewrote `tests/idempotency.rs::replaying_the_same_digest_returns_the_unchanged_outcome`
  to exercise the actual replay path (transactional `lookup` returns the
  stored outcome and no second insert happens) and additionally proves the
  unique key rejects a blind duplicate insert without touching the row.
- Added `tests/outbox_concurrent.rs`: barrier-synchronized tasks stage bursts
  on one stream, both one transaction per event (6x8) and one burst per
  transaction (4x6); allocations must cover `1..=total` exactly and the
  ordered scan must match.
- Added `tests/props.rs` with proptest cases (16 per property) for P2, P3, P4
  on fresh temp stores and multi-threaded runtimes:
  - P2 `cas_race_has_exactly_one_winner`: 2–6 writers race revision 0 with a
    generated patch; exactly one CAS returns `true`, persisted revision is 1,
    step equals the winner's patch.
  - P3 `concurrent_appends_are_contiguous`: 2–4 tasks, random bursts (1–5)
    and chunk sizes (1–3 events per transaction); allocations and the ordered
    scan are exactly `1..=total`.
  - P4 `replay_returns_the_stored_outcome_unchanged`: generated outcome code,
    payload, and replay count; every replay returns the stored record, and the
    row counts of all 30 inception-schema tables are identical before/after.
- `cargo fmt -p kernel-store-sqlite -p events` applied to the partially
  formatted implementation files.

### Semantics

- **Allocation is structural.** The head row is created at `1` or incremented
  in a single statement with `RETURNING`; there is no read-then-write window
  (D10). Allocation runs inside the caller's transaction, so a rollback
  returns the position (proved by `rollback_discards_outbox_rows_and_stream_allocations`).
- **Replay.** `lookup` runs in the caller's transaction; the caller compares
  the digest and returns the stored outcome on a match. `insert` does no
  pre-check: any second row for the same key is rejected by the unique
  constraint and maps to `Conflict`/`Never` through `mapping::from_sqlx` (D5).
- **Outbox.** Rows are immutable except `journal_published_at_ms` /
  `live_published_at_ms` (schema trigger); scans filter on the journal marker
  and order by `(stream_key, sequence)`; `stage` composes `allocate` +
  `insert_outbox` and does no I/O of its own.

## Acceptance criteria

| Criterion | Evidence | Command |
|---|---|---|
| R5.1–R5.4 / P4 — transactional lookup, stored outcome on replay with unchanged rows, digest mismatch conflicts | `idempotency::insert_then_lookup_returns_the_stored_record`, `lookup_inside_a_rolled_back_transaction_leaves_no_record`, `replaying_the_same_digest_returns_the_unchanged_outcome`, `same_key_with_a_different_digest_conflicts`; `props::replay_returns_the_stored_outcome_unchanged` (all 30 canonical table counts identical) | `cargo test -p kernel-store-sqlite --test idempotency --test props` |
| R5.5 — racing identical submissions yield one insert | `idempotency::concurrent_identical_submissions_produce_one_row` (6 submissions on a barrier, one insert, one row) | same |
| R6.1–R6.3 — in-transaction allocation, unique/immutable rows, ordered scans | `outbox::allocate_returns_contiguous_sequences_per_stream`, `duplicate_event_id_is_rejected`, `duplicate_stream_sequence_is_rejected`, `scan_unpublished_is_ordered_and_honors_the_limit`, `mark_published_updates_only_the_selected_marker`, `outbox_rows_reject_mutation_outside_publication_metadata` | `cargo test -p kernel-store-sqlite --test outbox` |
| R6.4 / P3 — concurrent appends are contiguous and unique | `outbox_concurrent::barrier_bursts_with_one_transaction_per_event_stay_contiguous`, `barrier_bursts_staged_in_one_transaction_stay_contiguous`; `props::concurrent_appends_are_contiguous` | `cargo test -p kernel-store-sqlite --test outbox_concurrent --test props` |
| R6.5 — outbox visibility matches the causing transaction | `outbox::rollback_discards_outbox_rows_and_stream_allocations` (allocation and row vanish together; retry reuses the position) | `cargo test -p kernel-store-sqlite --test outbox` |
| P2 — CAS race property holds | `props::cas_race_has_exactly_one_winner`; `contention::exactly_one_writer_wins_a_raced_cas` still green | `cargo test -p kernel-store-sqlite --test props --test contention` |
| N2 — no sleeps, barriers only | `rg "sleep"` over all changed files: no matches; concurrency tests use `tokio::sync::Barrier` exclusively | see Verification |
| No file outside `files:` changed | Commit contains exactly the 10 leased paths | `git show --stat 949feef` |

## RED / GREEN

RED — the four test targets were run against the pre-PST-005 tree (tracked
implementation files restored to HEAD with `UnavailableRepo`, the new
implementation files moved aside), then restored. All tests fail for the
expected reason; P2 passes because run CAS predates this task.

```
running 5 tests
test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
error: test failed, to rerun pass `-p kernel-store-sqlite --test idempotency`
running 7 tests
test result: FAILED. 0 passed; 7 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
error: test failed, to rerun pass `-p kernel-store-sqlite --test outbox`
running 2 tests
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
error: test failed, to rerun pass `-p kernel-store-sqlite --test outbox_concurrent`
running 3 tests
test cas_race_has_exactly_one_winner ... ok
test concurrent_appends_are_contiguous ... FAILED
test replay_returns_the_stored_outcome_unchanged ... FAILED
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.43s
error: test failed, to rerun pass `-p kernel-store-sqlite --test props`
```

RED panic (shim path):

```
thread 'insert_then_lookup_returns_the_stored_record' panicked at crates/kernel-store-sqlite/tests/idempotency.rs:108:10:
called `Result::unwrap()` on an `Err` value: KernelError { code: FailedPrecondition, retry: Never, message: "repository group IdempotencyRepo is not available", source: None }
```

RED — partial-work defect fixed in this task, before the fix:

```
---- rollback_discards_outbox_rows_and_stream_allocations stdout ----
thread 'rollback_discards_outbox_rows_and_stream_allocations' panicked at crates/kernel-store-sqlite/tests/outbox.rs:319:5:
assertion `left == right` failed
  left: 1
 right: 2
```

GREEN — `cargo test -p kernel-store-sqlite -p events`:

```
events:               1 passed
kernel_store_sqlite:  6 passed
bootstrap:            8 passed
cas:                  6 passed
contention:           2 passed
fence:                5 passed
idempotency:          5 passed
immutability:         4 passed
outbox:               7 passed
outbox_concurrent:    2 passed
props:                3 passed
repos_remaining:     12 passed
rollback:             7 passed
doc-tests:            0 passed
```

## Verification

From `agent-os/`:

- `cargo test -p kernel-store-sqlite -p events` — all green (counts above).
- `cargo test --workspace` — exit 0; 81 `test result: ok` lines; zero
  `FAILED`/`error[`.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- No `unwrap()`/`expect()`/`sleep` in the changed production files
  (`idempotency.rs`, `streams.rs`, `txn.rs`, `mod.rs`, `events/outbox.rs`);
  test files use them as allowed.
- `git show --stat 949feef` — exactly the 10 leased paths.
- No deferred-work markers (`TODO`/`FIXME`/`unimplemented!`) in any changed file.

## Concerns

1. **`events::outbox::stage` is not exercised end-to-end against a real
   transaction.** `events` is not a dev-dependency of `kernel-store-sqlite`
   (and `Cargo.toml` is outside this file lease), and `events` has no testkit
   dependency, so no integration test can call `stage` with a `SqliteWriteTxn`.
   Coverage today: the `DraftEvent -> NewOutboxEvent` projection unit test,
   plus the repository tests for the allocate/insert operations `stage`
   composes. Adding `events` as a dev-dependency of `kernel-store-sqlite`
   and calling `stage` from `tests/outbox.rs` would close this gap.
2. **`mapping::unavailable` is kept alive by a `const _` assertion.** With the
   shim gone the helper is unused; `mapping.rs` is outside the file lease, so
   `repos/mod.rs` links it instead. A follow-up that owns `mapping.rs` can
   delete both the helper and the assertion.
3. **`stage` timestamps are not injectable.** `events/src/outbox.rs` reads
   `SystemClock.now_unix_ms()` directly; the specified signature
   `stage(txn, draft)` carries no clock. Deterministic timestamp tests would
   need a clock parameter or a test seam (not required by any PST-005
   acceptance criterion).
4. **`scan_unpublished` intentionally filters only on the journal marker.**
   Live-published but unjournaled rows remain in the scan, as
   `mark_published_updates_only_the_selected_marker` asserts. This matches
   R6.3/design; noted so consumers are not surprised.
