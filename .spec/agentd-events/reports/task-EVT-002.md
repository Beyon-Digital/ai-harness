# Task EVT-002 — Event Journal port and SQLite journal

- Status: in review
- Agent: agent-evt002
- Spec: agentd-events
- Commit: `7150dea` — feat(journal): sqlite event journal with exact append rules [EVT-002]
- Branch: feat/agentd-microkernel-mvp
- Depends on: EVT-001 (`584cc63`, `20aab94`)

## What changed

| File | Change |
| --- | --- |
| `agent-os/crates/event-journal/src/lib.rs` | new: `AppendResult`, `ReadResult`, `EventJournalPort` per the design interface block |
| `agent-os/crates/event-journal-sqlite/src/lib.rs` | new: `JournalConfig`, `SqliteEventJournal::open`, `EventJournalPort` impl with `BEGIN IMMEDIATE`, embedded schema, mode/`busy_timeout` bootstrap, local sqlx→kernel mapping |
| `agent-os/crates/event-journal-sqlite/tests/journal.rs` | new: 11 integration tests written first (RED), covering append/read, idempotency, conflicts, gaps, retention flag, concurrency, private modes, redaction |

No manifest edits were needed: EVT-000 already declared every required dependency
(`async-trait`/`sqlx`/`tokio` in `event-journal-sqlite`, `async-trait` in
`event-journal`, dev-dep `tempfile`). No file outside the task's `files:` lease is
part of `7150dea`.

## Implementation notes

- **Port.** `AppendResult { final_sequence }` always names the last position of
  the batch (`expected_sequence + batch.len()`); for an idempotent replay that is
  the batch's own last position even when the stream head moved on.
  `ReadResult { events, retention_gap }` reports `retention_gap: false` as
  specified for the inception journal.
- **Separate database (R2.5).** `open` creates the runtime directory `0700` and
  `events.db` `0600` (`create_new` + re-asserted modes every open), bootstraps
  `include_str!("../../../schema/event_journal.sql")` verbatim only on first
  create, and shares no table names with `kernel.db`. The test asserts the table
  inventory is exactly `events`.
- **Connection options.** Mirrors `SqliteKernelStore`: `busy_timeout`, WAL,
  `synchronous=Full`, `foreign_keys=on`; pool of five connections. The
  `append_waits_out_a_held_writer_lock` test holds `BEGIN IMMEDIATE` on a second
  connection and proves the append waits instead of failing fast.
- **Append rules.** Batch validation first: non-empty, every envelope on the
  named stream, sequences exactly `expected_sequence + 1 …` (checked
  arithmetic). Then one `BEGIN IMMEDIATE` transaction: read head; if
  `head == expected_sequence`, insert the batch; otherwise require every batch
  position to be recorded and byte-identical (idempotent success, no write).
  Missing position → `FailedPrecondition`/`Never`; recorded position with a
  different event id (or different bytes for the same id) →
  `Conflict`/`Never`. The unique `(stream_key, sequence)` constraint remains the
  authority under races; sqlx constraint failures map through the persistence
  conventions (unique/PK → `Conflict`, other constraint →
  `FailedPrecondition`, BUSY/LOCKED/IOERR/CANTOPEN → `Unavailable`/`Safe`,
  validated with `classify_sqlite` unit-style logic local to this crate because
  `kernel-store-sqlite::mapping` is `pub(crate)`).
- **Reads.** `sequence > from_sequence` ascending with `LIMIT`, zero-copy row
  decode via `EventEnvelope::from_bytes`; stored `stream_key`, `sequence`, and
  `event_id` columns are cross-checked against the decoded envelope and mismatch
  fails closed with `Internal`/`Never`. Reading at or past the head is an empty
  success.
- **Redaction (N1).** No `unwrap()`/`expect()` outside tests; error messages
  never interpolate envelopes, and `KernelError::Display` never renders sources,
  so payload bytes cannot leak (test canary).
- No deferred-work markers.

## Interpretation decisions (spec-silent)

- A hole inside a batch, a batch that starts past `expected_sequence + 1`, an
  empty batch, or an envelope from another stream is a caller contract
  violation → `InvalidArgument`/`Never`.
- An expectation ahead of the stream head (batch contiguous from
  `expected_sequence + 1`, but those positions are not recorded) would leave a
  gap → `FailedPrecondition`/`Never`, matching "expected_sequence is the head
  before the batch".
- Mixed divergence resolves in position order: the first missing position is
  `FailedPrecondition`, the first recorded divergence is `Conflict`.
- Same event id at a position with different envelope bytes is `Conflict`
  (stricter than comparing event ids alone).

## Acceptance criteria

### R2.1–R2.4 — append rules, duplicates, conflicts, ordered reads — MET

```
$ cd agent-os
$ cargo test -p event-journal -p event-journal-sqlite
     Running tests/journal.rs (target/debug/deps/journal-c0e1c3915051f299)

running 11 tests
test batch_envelopes_must_belong_to_the_named_stream ... ok
test append_rejects_a_batch_that_would_leave_a_gap ... ok
test append_then_read_returns_ordered_events ... ok
test empty_batches_are_rejected ... ok
test concurrent_same_position_appends_have_exactly_one_winner ... ok
test duplicate_position_with_a_different_event_is_a_conflict ... ok
test errors_never_render_payload_bytes ... ok
test journal_file_is_private_and_does_not_share_kernel_tables ... ok
test non_contiguous_batches_are_rejected_before_writing ... ok
test exact_duplicate_append_is_idempotent ... ok
test append_waits_out_a_held_writer_lock ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
```

- expected sequence: `append_rejects_a_batch_that_would_leave_a_gap`,
  `non_contiguous_batches_are_rejected_before_writing`;
- idempotent exact duplicate: `exact_duplicate_append_is_idempotent` (including
  retry after later appends, no duplicate rows);
- conflict: `duplicate_position_with_a_different_event_is_a_conflict` (whole
  batch rejected when a tail conflicts);
- ordered reads + `retention_gap: false` + paging + read at/past head:
  `append_then_read_returns_ordered_events`.

### R2.5 — separate database, no canonical table touched — MET

`journal_file_is_private_and_does_not_share_kernel_tables` asserts the
`sqlite_master` table inventory is exactly `["events"]` on the new file.

### R2.6 / P2 — concurrent appends, contiguous sequences — MET

`concurrent_same_position_appends_have_exactly_one_winner` uses
`tokio::sync::Barrier` with no sleeps on a two-worker runtime: exactly one `Ok`
with `final_sequence == 1`, the loser is `Conflict`/`Never`, and the read shows
one contiguous event at sequence 1.

### N1 — `0600` file in a `0700` directory; payload never in errors — MET

Modes asserted with `PermissionsExt::mode` in
`journal_file_is_private_and_does_not_share_kernel_tables`; a distinct canary in
`errors_never_render_payload_bytes` never appears in `Display` or `Debug` of the
conflict and failed-precondition errors.

### No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' 7150dea
7150dea feat(journal): sqlite event journal with exact append rules [EVT-002]

 agent-os/crates/event-journal-sqlite/src/lib.rs    | 434 ++++++++++++++++++-
 .../crates/event-journal-sqlite/tests/journal.rs   | 479 +++++++++++++++++++++
 agent-os/crates/event-journal/src/lib.rs           |  65 +++
 3 files changed, 977 insertions(+), 1 deletion(-)
```

### Gates — MET

```
$ cargo test -p event-journal -p event-journal-sqlite        # exit 0
$ cargo clippy -p event-journal -p event-journal-sqlite --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.61s
    exit 0 (no warnings)
$ cargo fmt --check                                          # exit 0, empty output
```

## RED → GREEN

RED (`cargo test -p event-journal -p event-journal-sqlite`, before
implementation, exit 101):

```
error[E0432]: unresolved imports `event_journal::AppendResult`, `event_journal::EventJournalPort`
error[E0432]: unresolved imports `event_journal_sqlite::JournalConfig`, `event_journal_sqlite::SqliteEventJournal`
error: could not compile `event-journal-sqlite` (test "journal") due to 51 previous errors
```

An intermediate run exposed one wrong test expectation: appending sequence 2
with `expected_sequence = 0` is a contiguity violation (`InvalidArgument`), not
a head mismatch; the test was corrected to the real gap case (`expected`
ahead of an empty head → `FailedPrecondition`) before GREEN.

GREEN: 11 passed, 0 failed (output above), exit 0. Clippy and fmt clean after
replacing the one-element `clone()` slices flagged by
`clippy::cloned_ref_to_slice_refs` with `std::slice::from_ref`.

## Files committed

- `agent-os/crates/event-journal/src/lib.rs`
- `agent-os/crates/event-journal-sqlite/src/lib.rs`
- `agent-os/crates/event-journal-sqlite/tests/journal.rs`

## Concerns

- **Error split is an interpretation.** The brief pins
  `FailedPrecondition`/`Never` to an `expected_sequence` mismatch and
  `Conflict`/`Never` to a same-position different-id, but does not assign a code
  to a non-contiguous batch; this task uses `InvalidArgument`/`Never` and
  documents it above. Mixed divergence resolves in position order.
- **Empty batch** is rejected `InvalidArgument`/`Never`; the brief is silent and
  EVT-003 never sends one.
- **Extra accessor.** `SqliteEventJournal::path()` mirrors
  `SqliteKernelStore::path()` for diagnostics/tests; the design block lists only
  `open`.
- **Behavioral timeout test uses one 100 ms sleep** to prove the append waits on
  a held writer lock; the concurrency requirement itself is barrier-driven with
  no sleeps. If sleeps are later banned outright, this test can be replaced by a
  `busy_timeout` PRAGMA check on an exposed connection.
- `.spec/agentd-events/{ledger,tasks}.md` carry spec-flow bookkeeping from
  claim/start/review; they are outside this task's lease and are not part of
  `7150dea`.

---

# Fix report — review items (commit `251b9f7`)

- Status: in review (not done)
- Agent: agent-evt002
- Commit: `251b9f7` — fix(journal): repair schema bootstrap and drop timing-based test [EVT-002]

## Finding 1 — no wall-clock sleeps in tests

`append_waits_out_a_held_writer_lock` is replaced by
`append_fails_retry_safely_while_the_writer_lock_is_held`: the journal opens
with `busy_timeout_ms: 1`, a second connection holds `BEGIN IMMEDIATE`, the
append is attempted against that held lock and must return
`Unavailable`/`Safe` (SQLite BUSY after the timeout, which the test never
sleeps through), the stream stays empty, and a post-rollback append proves the
failure was lock contention and not a poisoned pool.

```
$ grep -rn "sleep" agent-os/crates/event-journal-sqlite/tests/
$ echo $?
1
```

The concurrency test remains barrier-driven and unchanged.

## Minor 1 — bootstrap repairs an empty database file

`open` no longer gates the schema on `create_new` winning. `ensure_schema`
connects, checks `sqlite_master` for the `events` table, and executes the
embedded schema verbatim only when it is absent; permissions and `busy_timeout`
handling are unchanged. Note: the embedded file uses plain `CREATE TABLE`
(no `IF NOT EXISTS`), so the existence check is the guard.

New regression test `open_bootstraps_an_existing_but_empty_database_file`
writes a zero-byte `events.db` (the crash window) and asserts append/read
work after open. RED probe with the repair disabled:

```
test open_bootstraps_an_existing_but_empty_database_file ... FAILED
panicked at ...: append succeeds after repair: KernelError { code: Internal,
retry: Never, message: "event journal storage operation failed",
source: Some(Database(SqliteError { code: 1, message: "no such table: events" })) }
test result: FAILED. 0 passed; 1 failed; ... 12 filtered out
```

## Minor 2 — same id/position, different bytes conflicts

New test `same_position_and_event_id_with_different_bytes_is_a_conflict`
re-appends the recorded `event_id` at the recorded position with different
envelope bytes (mutated payload/time; `assert_ne!` on `to_bytes`) and asserts
`Conflict`/`Never` with the stored event unchanged.

## Minor 3 — canary assertion is no longer vacuous

`errors_never_render_payload_bytes` now builds a canary-bearing envelope on a
fresh stream and triggers the `FailedPrecondition` gap rejection while that
envelope is in the batch, asserting `Display` and `Debug` of that error never
contain the canary; the original conflict-path assertions stand.

## Minor 4 — `prepare_runtime_dir` precondition documented

The function now carries a doc comment stating it creates and chmods its
argument to `0700` and that callers must pass the dedicated runtime root
(`events.db`'s parent), never a shared or user-owned directory.

## Gates

```
$ cd agent-os
$ cargo test -p event-journal -p event-journal-sqlite
running 13 tests
...
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s

$ cargo clippy -p event-journal -p event-journal-sqlite --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.72s   # exit 0

$ cargo fmt --check                                                       # exit 0, empty output
```

## Files committed

```
$ git show --stat --format='%h %s' 251b9f7
251b9f7 fix(journal): repair schema bootstrap and drop timing-based test [EVT-002]

 agent-os/crates/event-journal-sqlite/src/lib.rs    |  87 +++++++++------
 agent-os/crates/event-journal-sqlite/tests/journal.rs | 120 +++++++++++++++++----
 2 files changed, 155 insertions(+), 52 deletions(-)
```

(`event-journal/src/lib.rs` from `7150dea` needed no change in this round.)

## Open concerns after fixes

- The previous "100 ms sleep" concern is resolved; no sleeps remain anywhere in
  the journal tests.
- Error-code and empty-batch interpretations from the original report stand
  (documented above).
- `.spec/agentd-events/{ledger,tasks}.md` and the review diff/report artifacts
  are outside the task lease and are not part of `251b9f7`.

