# Design — agentd-events

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

## Architecture

```
 commands ──stage──▶ kernel.db outbox ──scan──▶ EventDispatcher ──append──▶ events.db journal
                                                        │                        │
                                                        │  after ack             │ read/resume
                                                        ▼                        ▼
                                                   LiveBus ──▶ subscribers ──▶ EventCursor
                                                        │
                                                        └──▶ EphemeralBus (lossy, non-durable)
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| Primitives | Envelope builder, stream keys, cursors, classification policy | new fill of stubs | `agent-os/crates/events/src/{envelope,stream,cursor}.rs` |
| Journal port | Append/read contract with expected-sequence semantics | new fill | `agent-os/crates/event-journal/src/lib.rs` |
| Journal SQLite | `events.db` bootstrap, append rules, reads | new fill | `agent-os/crates/event-journal-sqlite/src/lib.rs` |
| Dispatcher | Outbox scan, journal-first append, publication marks, bus hand-off | new | `agent-os/crates/events/src/dispatcher.rs` |
| Live bus | Bounded durable delivery with explicit lag; lossy ephemeral channel | new | `agent-os/crates/events/src/live_bus.rs` |
| Outbox worker | Interval loop around one dispatch iteration | new fill | `agent-os/crates/agentd/src/workers/outbox.rs` |

## Data flow

**Happy path — outbox to subscriber**

1. A command transaction has already committed outbox rows with contiguous `(stream_key, sequence)` positions.
2. The worker calls `EventDispatcher::dispatch_once`.
3. `KernelStore::begin_read` is used by `scan_unpublished(limit)`, ordered by stream and sequence.
4. Rows are grouped per stream; for each group the dispatcher computes `expected_sequence = first_sequence - 1` and calls `EventJournalPort::append`.
5. The journal validates the expected head and appends; duplicates by event id at the same position are idempotent.
6. After a successful append the dispatcher calls `StreamRepo::mark_published(event_id, Journal)` inside a short write transaction.
7. It then hands each event to `LiveBus::publish`, which broadcasts to subscribers in order.
8. Subscribers receive `LiveItem::Event` and may advance their cursor to the event's cursor.

**Crash path — append succeeded, mark missing**

1. The journal contains the events but the outbox rows remain unpublished.
2. After restart the dispatcher re-appends the same batch; the journal returns idempotent success for identical `(event_id, stream, sequence)`.
3. The publication mark is written, and no duplicate history exists (P1).

**Failure path — journal unavailable**

1. `append` returns `Unavailable` with retry-safe classification.
2. The dispatcher records the failure and returns an outcome with a non-zero backlog; no mark and no live delivery.
3. The worker retries with capped backoff; commands continue committing because they never touch the journal.

**Failure path — slow subscriber**

1. The broadcast buffer overflows for one subscriber.
2. The subscription receives `RecvError::Lagged`, reports `LiveItem::Lagged { resume_from }` with the last journal-backed cursor it had delivered, and ends.
3. The client resumes through `EventJournalPort::read_stream` from that cursor; no cursor is fabricated.

## Interfaces

### events :: stream.rs

```rust
pub enum StreamKind { Run, Task, Session, Effect, ConfigGlobal, Adapter, Principal }

pub struct StreamKey(domain::ids::EventStreamKey);
impl StreamKey {
    pub fn run(id: domain::ids::RunId) -> Self;
    pub fn task(id: domain::ids::TaskId) -> Self;
    pub fn session(id: domain::ids::SessionId) -> Self;
    pub fn effect(id: domain::ids::EffectId) -> Self;
    pub fn config_global() -> Self;
    pub fn adapter(id: domain::ids::AdapterId, version: &str, digest: &str) -> errors::Result⟨Self⟩;
    pub fn principal(id: domain::ids::PrincipalId) -> Self;
    pub fn kind(&self) -> StreamKind;
    pub fn as_str(&self) -> &str;
}
impl std::str::FromStr for StreamKey { type Err = errors::KernelError; }
impl std::fmt::Display for StreamKey;
```

### events :: cursor.rs

```rust
pub use domain::ids::EventCursor;                 // v1:{stream_key}:{sequence}, opaque at boundaries
impl EventCursor { pub fn for_event(key: &StreamKey, sequence: u64) -> Self; }
```

### events :: envelope.rs

```rust
pub trait ClassificationPolicy: Send + Sync {
    /// Minimum sensitivity the event type may carry.
    fn minimum(&self, event_type: &str) -> Option⟨domain::security::SensitivityClass⟩;
    /// Default retention declared by the catalog for the event type.
    fn default_retention(&self, event_type: &str) -> Option⟨domain::security::RetentionClass⟩;
}

pub struct CatalogClassificationPolicy { /* parsed once from the embedded catalog */ }
impl CatalogClassificationPolicy {
    /// Parses `agent-os/proto/events/catalog.yaml`, embedded at compile time.
    pub fn embedded() -> errors::Result⟨Self⟩;
}
impl ClassificationPolicy for CatalogClassificationPolicy;

pub struct EventBuilder { /* ... */ }
impl EventBuilder {
    pub fn new(event_type: impl Into⟨String⟩, event_version: u32, stream_key: StreamKey) -> Self;
    pub fn sequence(self, sequence: u64) -> Self;
    pub fn occurred_at_ms(self, ms: i64) -> Self;
    pub fn correlation_id(self, id: impl Into⟨String⟩) -> Self;
    pub fn causation_id(self, id: impl Into⟨String⟩) -> Self;
    pub fn run_id(self, id: domain::ids::RunId) -> Self;      // plus task/session/effect setters
    pub fn payload(self, bytes: Vec⟨u8⟩) -> Self;
    /// Validates required fields and the classification floor from `policy`.
    pub fn build(self, policy: &dyn ClassificationPolicy) -> errors::Result⟨EventEnvelope⟩;
}

pub struct EventEnvelope { /* mirrors contracts/events/event.proto exactly */ }
impl EventEnvelope {
    pub fn to_bytes(&self) -> Vec⟨u8⟩;
    pub fn from_bytes(bytes: &[u8]) -> errors::Result⟨Self⟩;
    pub fn cursor(&self) -> EventCursor;
}
```

### event-journal port

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendResult { pub final_sequence: u64 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult { pub events: Vec⟨events::EventEnvelope⟩, pub retention_gap: bool }

#[async_trait::async_trait]
pub trait EventJournalPort: Send + Sync {
    async fn append(
        &self,
        stream_key: &events::StreamKey,
        expected_sequence: u64,
        batch: &[events::EventEnvelope],
    ) -> errors::Result⟨AppendResult⟩;

    async fn read_stream(
        &self,
        stream_key: &events::StreamKey,
        from_sequence: u64,
        limit: u32,
    ) -> errors::Result⟨ReadResult⟩;
}
```

### event-journal-sqlite

```rust
pub struct JournalConfig { pub path: std::path::PathBuf, pub busy_timeout_ms: u64 }
pub struct SqliteEventJournal { /* pool */ }
impl SqliteEventJournal { pub async fn open(config: JournalConfig) -> errors::Result⟨Self⟩; }
impl EventJournalPort for SqliteEventJournal;   // BEGIN IMMEDIATE, embedded schema
```

### events :: live_bus.rs

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveItem {
    Event(events::EventEnvelope),
    /// The subscription fell behind; resume the journal from this cursor.
    /// `None` means no event was ever delivered: resume from stream inception.
    Lagged { resume_from: Option⟨events::EventCursor⟩ },
    /// The bus was dropped or closed; the subscription ends without a cursor.
    BusClosed,
}

pub struct LiveBus { /* tokio broadcast sender, capacity */ }
impl LiveBus {
    pub fn new(capacity: usize) -> Self;
    pub fn publish(&self, event: &events::EventEnvelope);
    pub fn subscribe(&self) -> LiveSubscription;
    pub fn subscriber_count(&self) -> usize;
}
impl crate::dispatcher::LiveSink for LiveBus;   // the dispatcher's hand-off seam (EVT-004)
pub struct LiveSubscription { /* receiver + last delivered cursor */ }
impl LiveSubscription {
    pub async fn next(&mut self) -> LiveItem;
}

pub struct EphemeralBus { /* bounded lossy channel + AtomicU64 drops */ }
impl EphemeralBus {
    pub fn new(capacity: usize) -> Self;
    pub fn try_publish(&self, bytes: Vec⟨u8⟩) -> bool;   // false when dropped
    pub fn dropped(&self) -> u64;
}
```

### events :: dispatcher.rs

```rust
pub const AFTER_JOURNAL_APPEND: &str = "outbox.after_journal_append";

pub struct DispatchOutcome { pub scanned: usize, pub published: usize, pub backlog: usize }

pub struct EventDispatcher { /* store, journal, sink, faults, clock, ids, system principal */ }
pub trait LiveSink: Send + Sync {
    fn publish(&self, event: &events::EventEnvelope);
}
impl EventDispatcher {
    pub fn new(
        store: std::sync::Arc⟨dyn kernel_store::KernelStore⟩,
        journal: std::sync::Arc⟨dyn event_journal::EventJournalPort⟩,
        sink: std::sync::Arc⟨dyn LiveSink⟩,
        faults: std::sync::Arc⟨dyn domain::faults::FaultInjector⟩,
        clock: std::sync::Arc⟨dyn domain::time::Clock⟩,
        ids: std::sync::Arc⟨dyn domain::provider::IdProvider⟩,
        system_principal: domain::ids::PrincipalId,
    ) -> Self;
    /// One deterministic iteration: scan, append per stream, mark, publish.
    pub async fn dispatch_once(&self, limit: u32, daemon_epoch: u64)
        -> errors::Result⟨DispatchOutcome⟩;
}
```

## Data model

`events.db` uses the inception journal schema verbatim (`agent-os/schema/event_journal.sql`):
`events(event_id PK, stream_key, sequence, event_type, event_version, occurred_at_ms, envelope BLOB, UNIQUE(stream_key, sequence))`.
The dispatcher updates only the publication markers of `outbox_events` in `kernel.db`
(`journal_published_at_ms`, `live_published_at_ms`), permitted by the immutability trigger.
No migration. The one schema edit in this module is EVT-G0's retention CHECK narrowing (1-3)
and its comment update.

## Error handling

| Failure mode | Detection | Response | Serves |
|---|---|---|---|
| Expected-sequence gap | journal head differs from expected | `FailedPrecondition`, `Never`; no rows written | R2.1 |
| Same position, different event id | unique conflict on `(stream_key, sequence)` | `Conflict`, `Never` | R2.3 |
| Exact duplicate | event id and position match | idempotent success, no write | R2.2 |
| Journal file unreachable | open or statement IO error | `Unavailable`, `Safe`; dispatcher backs off | R3.4 |
| Classification downgrade | policy check in `build` | `FailedPrecondition`, `Never` | R1.4 |
| Malformed stream key or cursor | parse at construction | `InvalidArgument`, `Never` | R1.2, R1.3 |
| Unknown event type | policy has no entry | allowed with no floor; the catalog is the source of truth for catalogued types | R1.4 |
| Broadcast lag | `RecvError::Lagged` | `LiveItem::Lagged` with the last delivered cursor; subscription ends | R4.2 |

**Error taxonomy:** `errors` crate; no new codes.

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | n/a in-process; transport is API-003 |
| Input validation and injection | stream keys and cursors parse strictly; journal SQL is parameterized only |
| Secrets handling | payloads are opaque bytes, never rendered |
| Data exposure in logs / errors | events are referenced by type, stream, and sequence; byte payloads never logged (N1) |
| New network surface | none |
| File permissions | `events.db` `0600` in the `0700` runtime directory |
| Classification | raises allowed; downgrades below the catalog floor rejected; one spelling across proto, domain, catalog, and schema (EVT-G0) |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Unit | in-crate tests | `crates/events/src/{stream,cursor,envelope,live_bus}.rs` | parsing, builder validation, lag mechanics |
| Integration | cargo tests with temp DBs | `crates/event-journal-sqlite/tests/journal.rs` | append/read, duplicates, conflicts, gaps, retention flag |
| Integration | real store + real journal | `crates/events/tests/dispatcher.rs` | crash re-append, backlog, per-stream order, journal-first rule |
| Live bus | deterministic capacities | `crates/events/tests/live_bus.rs` | slow subscriber lag, resume, ephemeral drops |
| Regression | gates and pack validators | repo root and `agent-os/` | G1, G2, N3 |

**Property tests**

| Property | Statement | Generator strategy |
|---|---|---|
| P1 | any crash between append and mark yields exactly one journal row per event | loop over batch positions with the fault armed after append; assert row counts and then complete the dispatch |
| P2 | any append interleaving on one stream is contiguous | sequential batches with deliberate duplicate re-appends asserting head arithmetic |
| P3 | no delivery precedes journal acceptance | dispatcher tests assert journal rows exist before the bus receives (ordered assertions) |

**Explicitly not tested (and why):**

- Retention deletion — out of scope.
- Cross-process dispatch — one daemon owns the store.
- Wire transport encoding — API-003.

## Observability

| Signal | Where | Content |
|---|---|---|
| `DispatchOutcome` | worker logs | scanned, published, backlog counts |
| `EphemeralBus::dropped` | metric counter | lossy drops, never durable events |
| Dispatcher errors | error codes | stream key and sequence, never payload bytes |

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N2 | `dispatch_once` drives deterministic iterations; worker owns the interval | tests call dispatch_once directly |
| Bounded memory | fixed broadcast capacity and bounded poll batches | lag tests force overflow deterministically |

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | Journal-first with idempotent re-append; no cross-DB transaction | 2PC; journal write inside the kernel txn | only safe option across two SQLite files; the journal is downstream | R3.2, R3.3, R2.2 |
| D2 | `dispatch_once` plus a thin interval worker | testable sleeps; a blocking loop | deterministic tests without wall-clock waits | N2 |
| D3 | Classification floor from the embedded catalog parsed at build | hard-coded table; no policy | one source of truth and a real floor | R1.4, R1.5 |
| D4 | Broadcast plus explicit lag results carrying the last delivered cursor | unbounded channel; silent skip | bounded memory; no fabricated cursors | R4.2, R4.5 |
| D5 | Separate lossy ephemeral channel | reuse the durable bus | ephemeral loss must never disturb durable delivery | R4.4 |
| D6 | Retention classes are three (1-3) and the outbox CHECK narrows accordingly | keeping four with a mapping | ends the vocabulary divergence | R1.5 |
| D7 | Expected sequence is the first row's sequence minus one | querying a journal head | the outbox is the upstream source; idempotent duplicates cover replay | R3.1, R3.5 |
| D8 | Publication marks are written through the existing `StreamRepo` | a new store method | the port already exposes `mark_published` | R3.2 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1.1–R1.6 | stream/cursor/envelope modules, policy | unit and primitives tests |
| R2.1–R2.6 | journal port and SQLite implementation | `tests/journal.rs` |
| R3.1–R3.6 | dispatcher and worker | `tests/dispatcher.rs`, fault point |
| R4.1–R4.5 | live bus and ephemeral channel | `tests/live_bus.rs` |
| N1 | classification floor, redaction, file modes | unit tests and bootstrap test |
| N2 | `dispatch_once`, small capacities | test targets |
| N3 | reconciliation only | pack validators and mirror check |
| P1, P2, P3 | crash and ordering tests | dispatcher and journal tests |
| G1, G2 | gates and validators | repo root and `agent-os/` |

## File structure

Paths relative to `agent-os/` unless the pack path is explicit.

| Path | Create or modify | Responsibility | Owner task |
|---|---|---|---|
| `crates/events/Cargo.toml`, `crates/event-journal/Cargo.toml`, `crates/event-journal-sqlite/Cargo.toml`, `crates/agentd/Cargo.toml`, `Cargo.lock` | modify | dependency declarations | EVT-000 |
| `agent-os-microkernel-mvp-buildpack/contracts/events/event.proto` | modify | rename sensitivity/retention values | EVT-G0 |
| `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql` | modify | retention CHECK 1-3 and comments | EVT-G0 |
| `agent-os-microkernel-mvp-buildpack/contract-lock.sha256`, `MANIFEST.json` | regenerate | lock and manifest | EVT-G0 |
| `proto/events/event.proto`, `schema/kernel_store.sql` | modify | mirror refresh | EVT-G0 |
| `crates/domain/src/security.rs` | modify | variant renames | EVT-G0 |
| `crates/events/src/outbox.rs`, `crates/command-coordinator/tests/coordinator.rs`, `crates/kernel-store-sqlite/tests/{repos_remaining,outbox,props}.rs` | modify | usage updates for renamed variants | EVT-G0 |
| `crates/events/src/stream.rs`, `cursor.rs` | create | stream keys and cursor wiring | EVT-001 |
| `crates/events/src/envelope.rs` | create | builder, policy, envelope codec | EVT-001 |
| `crates/events/src/lib.rs` | modify | module declarations and re-exports | EVT-001, EVT-003, EVT-004 |
| `crates/events/tests/primitives.rs` | create | round trips and validation tests | EVT-001 |
| `crates/event-journal/src/lib.rs` | create | port types and trait | EVT-002 |
| `crates/event-journal-sqlite/src/lib.rs` | create | SQLite journal | EVT-002 |
| `crates/event-journal-sqlite/tests/journal.rs` | create | append/read suite | EVT-002 |
| `crates/events/src/dispatcher.rs` | create | dispatcher | EVT-003 |
| `crates/events/tests/dispatcher.rs` | create | crash, backlog, order tests | EVT-003 |
| `crates/agentd/src/workers/outbox.rs` | modify | interval worker | EVT-003 |
| `crates/events/src/live_bus.rs` | create | live and ephemeral buses | EVT-004 |
| `crates/events/tests/live_bus.rs` | create | lag and resume tests | EVT-004 |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
