# Task EVT-004 — Bounded live bus with explicit lag

- Status: in review
- Agent: agent-evt004b
- Spec: agentd-events
- Commit: `f9d6afb` — feat(events): bounded live bus with explicit lag [EVT-004]
- Branch: feat/agentd-microkernel-mvp
- Depends on: EVT-003 (`37f61f6`, `c07edd0`)

## What changed

| File | Change |
| --- | --- |
| `agent-os/crates/events/src/live_bus.rs` | new: `LiveItem`, `LiveBus`, `LiveSubscription`, `LiveSink` impl, `EphemeralBus` |
| `agent-os/crates/events/src/lib.rs` | `pub mod live_bus;`, re-export `LiveItem` |
| `agent-os/crates/events/tests/live_bus.rs` | new: 6 integration tests written first (RED), real `SqliteEventJournal` in a temp root |
| `agent-os/crates/events/Cargo.toml` | dev-dependency repair: `event-journal-sqlite.workspace = true` (append-only) |
| `agent-os/Cargo.lock` | refreshed `events` edge to include `event-journal-sqlite` |

## Implementation notes

- **Dev-edge repair (the prior block).** `event-journal-sqlite.workspace = true`
  was appended to `[dev-dependencies]`. Cargo accepted the dev-only cycle
  (`events(dev) -> event-journal-sqlite -> events`) with no rejection:
  `cargo check --workspace` exited 0 and the lock gained exactly one line
  (`+ "event-journal-sqlite"` under the `events` package). The fallback
  test-local adapter was not needed.
- **Broadcast mechanics.** `LiveBus` owns a `tokio::sync::broadcast::Sender`
  with the configured capacity; `publish` clones the envelope into the channel
  and ignores the no-subscriber send error (the event stays durable in the
  journal and nothing is fabricated for live delivery). `subscriber_count` is
  `receiver_count()`, so it tracks live registrations exactly.
- **Lag is capacity-driven.** `LiveSubscription` keeps
  `last_delivered: Option<EventCursor>`, updated only after an `Ok` receive
  from `event.cursor()`. On `RecvError::Lagged` it returns exactly one
  `LiveItem::Lagged { resume_from }` built from that stored cursor and sets
  `ended`; every later `next` panics with an explicit message. No cursor is
  ever synthesized, and the resume cursor is by construction journal-backed
  (`EventEnvelope::cursor()`), so a journal read strictly after it recovers
  the missed range with no gap and no duplicate.
- **Lag before first delivery.** There is no journal-backed cursor to report,
  so `next` panics rather than inventing one (the design's `Lagged` payload is a
  non-optional `EventCursor`); the test
  `lag_before_any_delivery_never_fabricates_a_cursor` pins this.
- **Dispatcher seam.** `impl LiveSink for LiveBus` delegates to the inherent
  `publish`, so EVT-003's `Arc<dyn LiveSink>` hand-off works unchanged.
- **Ephemeral channel.** `EphemeralBus` wraps a bounded `tokio::sync::mpsc`
  channel with a `try_send`-based `try_publish` and an `AtomicU64` drop
  counter. `Full` and `Closed` both count as drops. It shares no state with
  `LiveBus`: durable delivery and cursors are untouched by ephemeral overflow.
- **Clippy.** `LiveItem` is intentionally asymmetric per the design
  (`Event(EventEnvelope)` inline for allocation-free normal delivery); the
  workspace already uses a targeted `#[allow(clippy::large_enum_variant)]` in
  `domain/src/generated.rs`, and the same scoped allow is used here.

## Interpretation decisions (spec-silent)

- **No-cursor lag panics.** The design fixes `Lagged { resume_from:
  EventCursor }` with no terminal variant; a lag with no prior delivery has no
  honest value, so the subscription terminates by panic (documented on `next`).
- **Ephemeral consumer side.** The design block exposes only `new`,
  `try_publish`, and `dropped`; the receiver is retained inside the bus so the
  buffer holds real capacity. No consumer accessor was invented beyond the
  design surface.
- **Bus drop.** Dropping the `LiveBus` closes a live subscription; the closed
  receive path also panics, matching the terminal-state policy (documented).

## Acceptance criteria

### R4.1–R4.3 — ordered delivery, explicit lag with resume cursor, resumable with no gap or duplicate — MET

- `subscribers_receive_published_events_in_order` publishes three events and
  asserts delivery in publication order plus `subscriber_count` accounting.
- `slow_subscriber_reports_lag_with_its_last_delivered_cursor` runs capacity 2:
  the slow subscription delivers `e1` then receives
  `Lagged { resume_from: e1.cursor() }` after the buffer evicts `e2`, while the
  healthy subscription consumes `e2..=e5` throughout; a spawned second `next`
  on the slow subscription asserts the panic (terminal state).
- `resume_from_lag_reads_exactly_the_missed_journal_events` appends `e1..=e5`
  to a real `SqliteEventJournal` (temp runtime root), lags a capacity-1
  subscription at `e1`, then reads from `resume_from.sequence`: the result is
  exactly `e2..=e5`, sequences `[2,3,4,5]`, every sequence strictly after the
  cursor, and no duplicate event ids (`retention_gap` false).

### R4.4 — ephemeral drops counted, durable delivery unaffected — MET

`ephemeral_overflow_counts_drops_and_spares_the_durable_bus`: capacity-2
`EphemeralBus` accepts two messages, rejects the next two
(`dropped() == 2`), and while still overflowing a separate `LiveBus`
subscription receives its published event; one further rejected publish moves
`dropped()` to 3.

### R4.5 / P3 — no cursor advancement for undelivered events — MET

`last_delivered` changes only on `Ok` receipt before `LiveItem::Event` is
returned; the lag assertions show `resume_from` equals the last delivered
cursor, never the published tail. `publish` never touches cursors or the
journal.

### N2 — deterministic capacity-driven lag; no sleeps — MET

Lag arises from buffer capacity plus manual publish/receive interleaving; there
are no sleeps, timers, or barriers. `tokio::time::timeout` appears only around
`next()` in the test helper to bound an expected receipt.

### Gates — MET

```
$ cd agent-os
$ cargo test -p events
running 1 test  ... test result: ok. 1 passed; 0 failed
running 6 tests ... test result: ok. 6 passed; 0 failed   (live_bus)
running 11 tests ... test result: ok. 11 passed; 0 failed (primitives)
running 8 tests ... test result: ok. 8 passed; 0 failed   (dispatcher)
running 0 tests ... test result: ok. 0 passed; 0 failed   (doc-tests)

$ cargo test --workspace
cargo_exit=0
86 suites, 222 passed, 0 failed

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 05s
    exit 0 (no warnings)

$ cargo fmt --all --check
    FMT_CLEAN
```

### No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' f9d6afb
f9d6afb feat(events): bounded live bus with explicit lag [EVT-004]

 agent-os/Cargo.lock                      |   1 +
 agent-os/crates/events/Cargo.toml        |   1 +
 agent-os/crates/events/src/lib.rs        |   2 +
 agent-os/crates/events/src/live_bus.rs   | 177 +++++++++++++++++++++++
 agent-os/crates/events/tests/live_bus.rs | 231 +++++++++++++++++++++++++++++++
 5 files changed, 412 insertions(+)
```

## RED → GREEN

RED (`cargo test -p events --test live_bus`, before `live_bus.rs` existed):

```
error[E0432]: unresolved import `events::live_bus`
  --> crates/events/tests/live_bus.rs:18:13
   |
18 | use events::live_bus::{EphemeralBus, LiveBus, LiveItem, LiveSubscription};
   |             ^^^^^^^^ could not find `live_bus` in `events`

error[E0282]: type annotations needed
  --> crates/events/tests/live_bus.rs:56:5
   |
56 | /     tokio::time::timeout(Duration::from_secs(10), subscription.next())
57 | |         .await
   | |______________^ cannot infer type

Some errors have detailed explanations: E0282, E0432.
error: could not compile `events` (test "live_bus") due to 2 previous errors
```

GREEN:

```
$ cargo test -p events --test live_bus
running 6 tests
test live_bus_implements_the_dispatcher_sink_seam ... ok
test ephemeral_overflow_counts_drops_and_spares_the_durable_bus ... ok
test lag_before_any_delivery_never_fabricates_a_cursor ... ok
test subscribers_receive_published_events_in_order ... ok
test slow_subscriber_reports_lag_with_its_last_delivered_cursor ... ok
test resume_from_lag_reads_exactly_the_missed_journal_events ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s
```

The dev edge was verified before the RED run: `cargo check --workspace` exited
0 with the new `[dev-dependencies]` line and refreshed the `events` entry in
`agent-os/Cargo.lock`.

## Files committed

- `agent-os/crates/events/src/live_bus.rs`
- `agent-os/crates/events/src/lib.rs`
- `agent-os/crates/events/tests/live_bus.rs`
- `agent-os/crates/events/Cargo.toml`
- `agent-os/Cargo.lock`

## Concerns

- **No-cursor lag is a panic.** A subscription that overflows before any
  delivery cannot resume (the design carries a mandatory `EventCursor`), so it
  panics; clients that need a defined "cold start" should read the journal
  first and only then follow live events. A future `Lagged { resume_from:
  Option<EventCursor> }` or an explicit terminal variant would soften this.
- **`EphemeralBus` has no consumer accessor** in the design surface; the
  channel's receiver is retained internally, so the buffer is real and drops
  are counted, but nothing can drain it yet. Wiring a consumer will need an
  interface addition outside this task's scope.
- **`LiveItem` carries the envelope inline** (design shape), which is why the
  scoped `large_enum_variant` allow exists; boxing `Event` would remove the
  allow but add an allocation per delivered event and deviate from the design
  block.
- `.spec/agentd-events/{ledger,tasks}.md` carry spec-flow bookkeeping from
  claim/start/review; they are outside this task's lease and are not part of
  `f9d6afb`.

---

# Fix report — review items (commit `b8bf062`)

Review approved EVT-004 with one Important design gap and one sibling; the
design block was amended. Task left in review; not marked done.

## Important — cold-start lag panicked — FIXED

**Defect.** `Lagged` carried a mandatory `EventCursor`, so a subscription that
overflowed before delivering anything had no honest cursor and `next` panicked
(`live_bus.rs:121-123`), making a full-buffer cold start unrecoverable.

**Fix.** `LiveItem::Lagged { resume_from: Option<EventCursor> }` per the
amended design; the lag arm now ends the subscription and returns
`LiveItem::Lagged { resume_from: self.last_delivered.take() }` — `Some(cursor)`
for the normal case, `None` when no event was ever delivered (resume from
stream inception). No panic and no fabricated cursor remain.

**RED** (updated tests against the old API, exact compile failures):

```
error[E0308]: mismatched types
    |                                         ^^^^ expected `EventCursor`, found `Option<_>`
    |     = note: expected struct `EventCursor` found enum `Option<_>`

error[E0599]: no variant or associated item named `BusClosed` found for enum `LiveItem`
   --> crates/events/tests/live_bus.rs:162:60
    |
162 |     assert_eq!(receive(&mut subscription).await, LiveItem::BusClosed);
    |                                                            ^^^^^^^^^ variant or associated item not found
```

## Sibling — closed bus panicked — FIXED

**Defect.** `RecvError::Closed` panicked (`live_bus.rs:126-129`), so dropping
the bus aborted the subscriber instead of ending it.

**Fix.** `LiveItem::BusClosed` added per the amended design; the closed arm
returns it and ends the subscription. The new
`dropping_the_bus_closes_live_subscriptions` test drops the bus, asserts
`LiveItem::BusClosed`, and asserts the subscription is terminal.

## Minor — publish cloned with no receivers; ephemeral doc wording — FIXED

- `LiveBus::publish` now returns early when `receiver_count() == 0`, so no
  envelope clone is paid with zero subscribers. `publishing_without_subscribers_is_a_no_op`
  pins that a pre-subscription publish is neither delivered nor replayed.
- `EphemeralBus` doc corrected from "Overflow and withdrawal are counted" to
  "Dropped messages are counted", and `try_publish` now names the closed-channel
  drop path alongside the full-buffer one.

## Test updates

- `cold_start_lag_reports_no_resume_cursor` (renamed from
  `lag_before_any_delivery_never_fabricates_a_cursor`) asserts
  `Lagged { resume_from: None }` and terminal panic on the next call.
- `slow_subscriber_reports_lag_with_its_last_delivered_cursor` asserts
  `Some(events[0].cursor())`.
- `resume_from_lag_reads_exactly_the_missed_journal_events` pattern-matches
  `Some(resume_from)` from the real `SqliteEventJournal` resume read.
- New: `dropping_the_bus_closes_live_subscriptions`,
  `publishing_without_subscribers_is_a_no_op`.

## GREEN and gates

```
$ cd agent-os
$ cargo test -p events
running 1 test  ... test result: ok. 1 passed; 0 failed   (unit)
running 8 tests ... test result: ok. 8 passed; 0 failed   (live_bus)
running 8 tests ... test result: ok. 8 passed; 0 failed   (dispatcher)
running 11 tests ... test result: ok. 11 passed; 0 failed (primitives)
running 0 tests ... test result: ok. 0 passed; 0 failed   (doc-tests)

$ cargo test --workspace
workspace_exit=0
86 suites, 224 passed, 0 failed

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.23s
    exit 0 (no warnings)

$ cargo fmt --check
    FMT_CLEAN
```

## Files committed (`b8bf062`)

```
 agent-os/crates/events/src/live_bus.rs   | 56 +++++++++++++++++++-------------
 agent-os/crates/events/tests/live_bus.rs | 49 +++++++++++++++++++++++-----
 2 files changed, 74 insertions(+), 31 deletions(-)
```

## Open concerns after the fix

- **`None` resume means stream inception.** A cold-start lag gives the
  consumer no journal position, so it must resume by reading the stream from
  sequence 0; callers that want a bounded replay window must prime their
  subscription cursor before the buffer fills.
- **Bus closure is a distinct terminal state.** A `BusClosed` subscription
  still has no cursor; a consumer that needs resumable shutdown should track
  its own last cursor in addition to `LiveItem`.
- **`EphemeralBus` has no consumer accessor** in the design surface; the
  channel's receiver is retained internally, so the buffer is real and drops
  are counted, but nothing can drain it yet.
- **`LiveItem` carries the envelope inline** (design shape), kept by the
  scoped `large_enum_variant` allow.
- `.spec/agentd-events/{design,ledger,tasks}.md` carry the amended design and
  spec-flow bookkeeping; they are outside this task's lease and are not part of
  `b8bf062`.
