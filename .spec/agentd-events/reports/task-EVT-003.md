# Task EVT-003 — Journal-first dispatcher and outbox worker

- Status: in review
- Agent: agent-evt003b
- Spec: agentd-events
- Commit: `c07edd0` — feat(events): journal-first dispatcher and outbox worker [EVT-003]
- Branch: feat/agentd-microkernel-mvp
- Depends on: EVT-002 (`7150dea`, `251b9f7`)

## What changed

| File | Change |
| --- | --- |
| `agent-os/crates/events/src/journal.rs` | new: `EventJournalPort`, `AppendResult`, `ReadResult` (cycle repair ruling); method signatures are exactly the expansion `#[async_trait]` produces, so `SqliteEventJournal`'s macro impl still matches |
| `agent-os/crates/events/src/lib.rs` | `pub mod journal;`, `pub mod dispatcher;`, re-exports |
| `agent-os/crates/events/src/dispatcher.rs` | new: `AFTER_JOURNAL_APPEND`, `DispatchOutcome`, `LiveSink`, `EventDispatcher::new`, `dispatch_once` |
| `agent-os/crates/events/tests/dispatcher.rs` | new: 7 integration tests written first (RED), real `SqliteKernelStore` + test-local in-memory journal |
| `agent-os/crates/event-journal/src/lib.rs` | now re-exports `events::journal::{AppendResult, EventJournalPort, ReadResult}`; `event-journal-sqlite` and all existing imports compile unchanged |
| `agent-os/crates/agentd/src/workers/outbox.rs` | new: `EpochSource`, `OutboxWorker` — interval loop, capped backoff, `watch` cancellation |

## Implementation notes

- **Cycle repair (per the amended design).** The port moved from
  `event-journal` to `events/src/journal.rs`; `event-journal` re-exports it. The
  trait methods are declared with the exact desugared signature of
  `#[async_trait::async_trait]` (`Pin<Box<dyn Future + Send + 'async_trait>>`
  with `'life0…` bounds). Verified in a scratch crate that the macro-generated
  impl in `event-journal-sqlite` still satisfies the hand-declared trait and that
  `Arc<dyn EventJournalPort>` is object-safe; `cargo check --workspace` passes.
  This keeps `events` free of a macro dependency, which would otherwise close an
  impossible `events -> event-journal -> events` cycle.
- **Dispatch iteration.** `dispatch_once` (1) scans unpublished rows via
  `StreamRepo::scan_unpublished(limit)` in a short fenced write transaction
  opened from the passed `daemon_epoch`, the system principal, and a fresh
  command id, then rolls it back; (2) groups consecutive rows per stream
  (`expected_sequence = first_row.sequence - 1`, `saturating_sub` so sequence 1
  yields 0); (3) appends each stream batch, then consults
  `outbox.after_journal_append`; on true returns `Unavailable`/`Safe` before any
  mark; (4) marks every scanned row `PublishKind::Journal` in one short fenced
  write transaction (same epoch/principal/fresh command id); (5) hands the
  envelopes to `LiveSink::publish` in scan order.
- **Idempotent recovery.** Because the journal's `append` replays an identical
  recorded batch as a success and only `journal_published_at_ms IS NULL` rows
  are scanned, a crash after append but before mark re-appends the same bytes
  and produces no duplicate row (P1).
- **Worker.** `OutboxWorker::run` uses `tokio::time::interval(poll)` with
  `MissedTickBehavior::Delay`, doubles the wait on `ErrorCode::Unavailable` from
  50 ms up to 2 s, resets on success/other errors, and returns promptly when the
  `watch::Receiver<bool>` flips or the sender is dropped. No sleeps or timers in
  the dispatcher or tests; tests call `dispatch_once` directly.

## Interpretation decisions (spec-silent)

- **Scan transaction.** The task context says "scan via `KernelStore::begin_read`",
  but `KernelReadTxn` intentionally does not expose `StreamRepo` ("write-only in
  the port surface"), and the dispatcher holds `Arc<dyn KernelStore>`, so no
  read transaction can reach `scan_unpublished`. The scan therefore runs in a
  short fenced write transaction that is rolled back immediately; it never
  writes, and it validates the fencing epoch like the mark transaction. The
  alternative would have required a kernel-store port change outside the lease.
- **Fault granularity.** `inject(AFTER_JOURNAL_APPEND)` is consulted after each
  per-stream append; the first fire aborts the iteration before any mark or
  delivery, leaving earlier appended streams to replay idempotently.
- **`DispatchOutcome.backlog`.** Defined as rows scanned but left unpublished by
  this iteration, so it is 0 on every successful iteration. R3.4's "backlog
  grows" is observed on the store (`scan_unpublished` keeps returning the rows)
  while the journal is down, which the tests assert directly; `backlog` is for
  worker logs.
- **Clock use.** The injected clock stamps the `TxContext.correlation_id`
  (`outbox.dispatch.<unix_ms>`) on both transactions; no wire-visible time is
  invented. `IdProvider` supplies the fresh `CommandId` per transaction.
- **Marks.** Only `PublishKind::Journal` is written, exactly as the task step
  specifies; the design's data-model sentence also names
  `live_published_at_ms` (see concerns).
- **Poison rows.** A stored stream key that does not parse as a canonical
  `StreamKey` fails the iteration with `Internal`/`Never` rather than silently
  skipping the row; messages never include payload bytes.
- **Worker dead code.** `agentd`'s composition root (`main.rs`) is outside the
  lease and does not construct workers yet, so the module carries
  `#![allow(dead_code)]` with a note; this is the smallest way to keep
  `-D warnings` meaningful for everything else until the wiring task lands.

## Acceptance criteria

### R3.1 — ordered scan — MET

`multi_stream_backlog_keeps_per_stream_order` stages A1, B1, A2, B2 and asserts
the journal receives `(A1, A2)` and `(B1, B2)` in sequence order; the sink
delivers in the `(stream_key, sequence)` scan order (`scan_unpublished`'s
`ORDER BY`).

### R3.2 — mark after append; no mark on failure — MET

`happy_path_appends_marks_and_delivers_in_order` asserts `scan_unpublished` is
empty after the iteration; `crash_after_append_reappends_without_duplicates`
asserts the faulted iteration leaves all rows unmarked; the unavailable-journal
test asserts the row stays unpublished after `Unavailable`.

### R3.3 / P1 — crash re-append, exactly one row per event — MET

`crash_after_append_reappends_without_duplicates`: the armed fault fails the
first iteration with `Unavailable`/`Safe`, the journal already holds 3 rows, no
marks exist and nothing was delivered; the second iteration completes and the
journal still holds exactly 3 rows.

### R3.4 — backlog grows, commands keep committing — MET

`unavailable_journal_grows_the_backlog_and_commands_still_commit`: the failing
journal stub leaves 1 row unpublished, a new command commits through the real
store while the journal is down (backlog 2), and recovery publishes both events
exactly once.

### R3.5 — per-stream order for multi-row batches — MET

Covered by the per-stream sequence assertions above and by
`happy_path_appends_marks_and_delivers_in_order` (single stream, 3 rows).

### R3.6 / P3 — no delivery before journal acceptance — MET

`no_sink_delivery_precedes_journal_acceptance` uses a sink that checks the
journal at every `publish` call: 2 delivered, 0 missing. The crash test asserts
0 deliveries while the journal rows exist but are unmarked.

### N2 — deterministic iterations, no sleeps — MET

All seven tests call `dispatch_once` directly; no timers, sleeps, or barriers.
Test seams are `TestClock`, `DeterministicIds`, `ArmedFaults`.

### Gates — MET

```
$ cd agent-os
$ cargo test -p events
running 1 test ... test result: ok. 1 passed; 0 failed
running 7 tests ... test result: ok. 7 passed; 0 failed
running 11 tests ... test result: ok. 11 passed; 0 failed
running 0 tests ... test result: ok. 0 passed; 0 failed

$ cargo test --workspace
86 suites, 215 passed, 0 failed

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 23.64s
    exit 0 (no warnings)

$ cargo fmt --check
    exit 0, empty output
```

### No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' c07edd0
c07edd0 feat(events): journal-first dispatcher and outbox worker [EVT-003]

 agent-os/crates/agentd/src/workers/outbox.rs |  93 ++++
 agent-os/crates/event-journal/src/lib.rs     |  66 +--
 agent-os/crates/events/src/dispatcher.rs     | 188 +++++++
 agent-os/crates/events/src/journal.rs        |  81 +++
 agent-os/crates/events/src/lib.rs            |   3 +
 agent-os/crates/events/tests/dispatcher.rs   | 781 +++++++++++++++++++++++++++
 6 files changed, 1150 insertions(+), 62 deletions(-)
```

## RED → GREEN

RED (`cargo test -p events --test dispatcher`, before `dispatcher.rs` existed,
exit 101):

```
error[E0432]: unresolved import `events::dispatcher`
  --> crates/events/tests/dispatcher.rs:20:13
   |
20 | use events::dispatcher::{AFTER_JOURNAL_APPEND, DispatchOutcome, EventDispatcher, LiveSink};
   |             ^^^^^^^^^^ could not find `dispatcher` in `events`

error: could not compile `events` (test "dispatcher") due to 1 previous error
```

GREEN:

```
$ cargo test -p events --test dispatcher
running 7 tests
test empty_outbox_is_a_no_op ... ok
test multi_stream_backlog_keeps_per_stream_order ... ok
test happy_path_appends_marks_and_delivers_in_order ... ok
test crash_after_append_reappends_without_duplicates ... ok
test no_sink_delivery_precedes_journal_acceptance ... ok
test stale_epoch_rejects_the_iteration_before_any_append ... ok
test unavailable_journal_grows_the_backlog_and_commands_still_commit ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
```

The cycle repair was checked before the RED run: `cargo check --workspace`
passed with the port in `events` and the re-export in `event-journal`.

## Files committed

- `agent-os/crates/events/src/journal.rs`
- `agent-os/crates/events/src/dispatcher.rs`
- `agent-os/crates/events/src/lib.rs`
- `agent-os/crates/events/tests/dispatcher.rs`
- `agent-os/crates/event-journal/src/lib.rs`
- `agent-os/crates/agentd/src/workers/outbox.rs`

## Concerns

- **Port placement deviation (approved ruling).** `EventJournalPort` lives in
  `events` rather than `event-journal`. Trait identity and all consumer imports
  are unchanged (`event-journal` re-exports it), but the crate-map description
  ("event-journal owns the journal port") is now stale. The hand-desugared
  signatures are coupled to async-trait's current expansion; if the workspace
  later upgrades async-trait and its expansion changes, the port must be
  updated in lockstep (a macro re-export or a Cargo.toml dev/normal edge would
  remove the coupling).
- **Scan uses a write transaction.** `KernelReadTxn` exposes no `StreamRepo`,
  so the task's "scan via `begin_read`" is not implementable through the
  available ports; the scan is a rolled-back `BEGIN IMMEDIATE`. A future
  `StreamRead` port would let the scan be a true read transaction.
- **`live_published_at_ms` is not written.** The task step names only
  `mark_published(event_id, Journal)`, and the sink hand-off is infallible, so
  no Live mark is written; the design data-model sentence mentions both
  markers. EVT-004 (live bus) may need to settle who writes the live marker.
- **`DispatchOutcome.backlog`** is 0 on every successful iteration by
  definition; if the reviewer wants "remaining work after the batch", the
  dispatcher needs a second count query.
- **Worker is not process-tested** (as specified) and is unreachable from
  `main`; the per-function `#[allow(dead_code)]` on `new`/`run` disappears with
  the composition-root wiring.
- **No worker logging.** The design's observability table says the worker logs
  `DispatchOutcome`, but `agentd` has no `tracing` dependency and its manifest
  is outside the lease; the outcome is returned, not logged.
- `.spec/agentd-events/{ledger,tasks}.md` carry spec-flow bookkeeping from
  claim/start/review; they are outside this task's lease and are not part of
  `c07edd0`.

---

# Fix report — review items (commit `37f61f6`)

Review verdict: approved with one Important item and one cheap minor. Task left
in review; not marked done.

## Important — mixed-batch replay wedged recovery — FIXED

**Defect.** When a crash left journal rows recorded but unmarked and a command
then committed more rows for the same stream, the recovery scan returned a
superset batch. `expected_sequence = first.sequence - 1` pointed behind the
journal head inside the recorded prefix, so `append` rejected the whole batch
with `FailedPrecondition`/`Never` and the iteration aborted, blocking every
later stream forever.

**Fix (inside the dispatcher, no port change).** For each stream group the
dispatcher now reads the already-recorded prefix first:

- `read_stream(stream_key, expected_sequence, min(batch.len(), u32::MAX))`.
- `recorded_prefix` walks `recorded` and `batch` position by position in
  sequence order and stops at the first difference; a same-position entry with
  a different event id — or different bytes under the same id, matching the
  journal's EVT-002 replay rule — returns `Conflict`/`Never`.
- The unrecorded suffix is appended with `expected_sequence = last matched
  sequence`, or the original expected when nothing matched. A fully recorded
  batch skips the append (and the fault consultation) and proceeds straight to
  marks and delivery.
- `MemoryJournal::read_stream` now honors the `failing` flag like
  `append`, so the stub models a closed journal on both paths.

**RED** (new test against the unfixed dispatcher, exact wedge):

```
$ cargo test -p events --test dispatcher mixed_batch_recovery
running 1 test
test mixed_batch_recovery_reappends_only_the_unrecorded_suffix ... FAILED

---- mixed_batch_recovery_reappends_only_the_unrecorded_suffix stdout ----
thread '...' panicked at crates/events/tests/dispatcher.rs:570:10:
mixed-batch recovery succeeds: KernelError { code: FailedPrecondition,
retry: Never, message: "test journal expected sequence does not match the head",
source: None }

test result: FAILED. 0 passed; 1 failed; 0 measured; 7 filtered out
```

**GREEN:**

```
$ cargo test -p events --test dispatcher
running 8 tests
test mixed_batch_recovery_reappends_only_the_unrecorded_suffix ... ok
test happy_path_appends_marks_and_delivers_in_order ... ok
test empty_outbox_is_a_no_op ... ok
test crash_after_append_reappends_without_duplicates ... ok
test multi_stream_backlog_keeps_per_stream_order ... ok
test unavailable_journal_grows_the_backlog_and_commands_still_commit ... ok
test no_sink_delivery_precedes_journal_acceptance ... ok
test stale_epoch_rejects_the_iteration_before_any_append ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The new test arms the post-append fault, records stream A (2 rows), leaves
stream B unreached, commits a third row for A, then recovers: stream A's
journal is `[1, 2, 3]` with no duplicate rows, stream B appends and publishes,
all 4 rows are marked, and the sink receives 4 events.

## Minor — module-wide `#![allow(dead_code)]` narrowed — FIXED

The blanket module attribute was removed. The only dead-code allows left are
`#[allow(dead_code)]` on `OutboxWorker::new` and `OutboxWorker::run`; rustc
treats an allowed item as a live root, so the trait, struct, and constants
reached from those two functions no longer warn, while any future dead code in
the module still fails `-D warnings`. Verified by deleting the blanket
attribute first: clippy reported exactly the constants, `EpochSource`,
`OutboxWorker`, `new`, and `run`, and the two function-level allows cleared all
six without hiding anything else.

## Gates

```
$ cd agent-os
$ cargo test -p events
running 1 test  ... ok. 1 passed
running 8 tests ... ok. 8 passed
running 11 tests ... ok. 11 passed
running 0 tests ... ok. 0 passed

$ cargo test --workspace
86 suites, 216 passed, 0 failed

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.51s
    exit 0 (no warnings)

$ cargo fmt --check
    exit 0, empty output
```

## Files committed (`37f61f6`)

```
 agent-os/crates/agentd/src/workers/outbox.rs    |   6 +-
 agent-os/crates/events/src/dispatcher.rs        |  70 +++++++++++++++---
 agent-os/crates/events/tests/dispatcher.rs      | 105 +++++++++++++++++++++++++
 3 files changed, 166 insertions(+), 15 deletions(-)
```

## Open concerns after the fix

- The prefix read is one extra `read_stream` per stream group per iteration;
  the common happy path now pays a query before rendering the append.
- A genuinely corrupted stream (journal head beyond a missing position) still
  fails `FailedPrecondition` and re-blocks, which is deliberate: replay cannot
  repair a gap, and skipping would silently lose events.
- Remaining concerns from the original report stand (`live_published_at_ms`
  unwritten, worker logging absent, `backlog` 0 on success).
