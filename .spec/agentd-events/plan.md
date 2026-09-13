# Plan — agentd-events

**Status:** draft
**Date:** 2026-09-11

Module spec. Phase 1 input is the approved umbrella plan at
`.spec/agentd-microkernel-mvp/plan.md`. This module is fourth in the build order; foundation,
persistence, and command-core are complete and approved. Approvals below are recorded under the
user's standing instruction to proceed without per-step confirmation.

## Problem statement

Commands stage outbox rows, but nothing publishes them: there is no Event Journal, no
dispatcher, and no live subscription. Durable history does not exist, so a client cannot replay
events, and `events.db` remains an empty schema file. The foundation's final review also
deferred a vocabulary conflict to this module: `contracts/events/event.proto` names sensitivity
`PRIVATE` and retention `SESSION`/`DURABLE`, while the normative event catalog and schema use
`confidential` and `standard`/`audit`. Until reconciled, no runtime code can classify an event
consistently.

## Outcome

The event pipeline is real: validated envelopes with canonical stream keys and cursors;
a downstream `events.db` journal with exact append/idempotency rules; a journal-first dispatcher
that survives crashes and duplicate work; and a bounded live bus whose slow subscribers are
disconnected with a resumable journal cursor instead of silently skipping durable history. The
classification vocabulary has one spelling across proto, catalog, domain, and schema.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | The vocabulary is reconciled to the catalog: sensitivity `PUBLIC/INTERNAL/CONFIDENTIAL/SECRET` (wire 1-4) and retention `EPHEMERAL/STANDARD/AUDIT` (wire 1-3), with the outbox CHECK narrowed to 1-3 | Keeping four retention values would leave `DURABLE` unnamed by the catalog and require a mapping table |
| 2 | `events.db` lives next to `kernel.db` in the runtime root, created `0600` in the same `0700` directory | A different location changes the dispatcher configuration and tests |
| 3 | The dispatcher polls on the normative `events.dispatcher_poll_ms` (500 ms) with capped exponential backoff and never blocks command commits | A push-based trigger would change EVT-003's shape |
| 4 | The dispatcher consumes the additive fault seam (`FaultInjector::inject`) with a point between journal append and publication mark | Without it the crash-reappend test needs a different mechanism |
| 5 | Live bus capacity is the normative `events.live_buffer_events` (256); ephemeral telemetry uses a separate lossy channel | Smaller test capacities are used to force lag deterministically |
| 6 | Retention classes are recorded and validated but nothing deletes events in the MVP | A retention sweep would be a new module with its own authority |

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| Stream keys, append semantics, and live-bus policy are normative | `agent-os-microkernel-mvp-buildpack/specs/event-pipeline.md:6-49` | EVT-001/002/004 implement these exactly; no invented formats |
| The journal schema already exists as a mirrored file | `agent-os/schema/event_journal.sql`; `specs/event-journal-schema.sql` | EVT-002 bootstraps it verbatim; only the file's PRAGMAs and version handling are new |
| The port surface is fixed | `contracts/ports/event_journal.proto` (append with expected sequence, read with limit and retention-gap flag) | `EventJournalPort` mirrors this shape |
| The envelope fields are fixed | `contracts/events/event.proto` | `EventEnvelope` carries ids, classification, correlation/causation, and payload bytes |
| The outbox row shape is already persisted | `agent-os/crates/kernel-store/src/models.rs` (`OutboxEventRow`), `specs/kernel-store-schema.sql` (`outbox_events`) | The dispatcher reads `scan_unpublished` and marks publication through `StreamRepo` |
| The dispatcher's publication markers exist | `outbox_events.journal_published_at_ms`, `live_published_at_ms`; immutability trigger allows only those columns | Journal-first marking is enforceable at the storage layer |
| Classification types exist but diverge from the catalog | `agent-os/crates/domain/src/security.rs:8-27` (`Private`, `Session`, `Durable`), `specs/event-catalog.md:16-21` | EVT-G0 reconciles names and the CHECK range before primitives are built |
| The fault seam and store port are ready | CMD-001A and persistence reports | Dispatcher crash tests use `inject`; no new seams |
| Limits are normative | `specs/limits.yaml`: `events.live_buffer_events: 256`, `events.dispatcher_poll_ms: 500` | Defaults come from the file, overridable in tests |

## Existing conventions to follow

- All commands from `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Pack validators from the repo root; the contract mirror check must stay green after EVT-G0.
- No `unwrap()`/`expect()` outside tests; no payload bytes in errors or logs; no sleeps in
  tests; parameterized SQL only in the SQLite crate.
- Events carry classification; raises are allowed, downgrades below the type minimum are not.

## Approach

### Chosen

Six sequential tasks:

1. **EVT-000** — declare the dependencies for `events`, `event-journal`,
   `event-journal-sqlite`, and `agentd`.
2. **EVT-G0** — reconcile the classification vocabulary: rename proto and domain sensitivity
   `PRIVATE` to `CONFIDENTIAL`, retention to `EPHEMERAL/STANDARD/AUDIT` (three values), narrow
   the outbox CHECK to `1 AND 3`, refresh the mirror, regenerate the lock and manifest, and
   update every Rust usage.
3. **EVT-001** — primitives: stream-key constructors, `EventCursor` wiring, a validated event
   builder (type, version, classification, correlation/causation), and envelope encode/decode.
4. **EVT-002** — `EventJournalPort` and the SQLite `events.db`: expected-sequence append,
   idempotent identical duplicates, conflicting duplicates, gap rejection, read with limit and
   the retention-gap flag (false in inception).
5. **EVT-003** — the journal-first dispatcher and the `agentd` outbox worker: ordered poll,
   append, mark, then live hand-off; capped backoff; a fault point between append and mark.
6. **EVT-004** — the durable live bus: bounded broadcast, explicit lag result carrying a resume
   cursor, a separate lossy ephemeral channel, and no fabricated cursor advancement.

Dependency edges: EVT-000 → EVT-G0 → EVT-001 → EVT-002 → EVT-003 → EVT-004.

### Rejected

| Alternative | Why not |
|---|---|
| Keep four retention values and map `DURABLE` to `audit` | The catalog has three values; a silent mapping is exactly the divergence this module must end |
| Make the journal a second authority for run state | Decision D-005: the journal is downstream; the kernel store stays authoritative |
| Write journal rows inside the kernel transaction | Two databases; no 2PC. Journal-first with idempotent re-append is the designed crash-safe path |
| Use an unbounded channel for the live bus | A slow subscriber would grow memory without bound and could still lose ordering |
| Push-based dispatch from the command coordinator | Couples command commits to the journal; the pack requires commands to commit even when the journal is down |
| Instrument spans now | Persistence and command-core both deferred observability; no consumer yet |

## Scope

**In scope**

- EVT-000, EVT-G0, and EVT-001 through EVT-004 exactly as their briefs define them
- Tests: stream key round trips, malformed cursors, envelope round trip, append/read,
  duplicate exact/conflicting appends, gap rejection, crash re-append, journal-unavailable
  backlog, per-stream order, slow subscriber lag, resume from journal, ephemeral drop
- The vocabulary reconciliation across proto, domain, schema comments, mirror, lock, manifest

**Explicitly out of scope**

- Event API read/subscribe transport (API-003, control-api module)
- Remote relay projections and redaction rules (remote-access module)
- Retention deletion, compaction, or archival
- New event types beyond the 61 catalogued

## Capability map

Single capability — durable event pipeline. No decomposition; six sequential tasks.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| Vocabulary surgery touches many files and the lock | Medium | Workspace compile, mirror check, validator | One task owns every affected file; lock and manifest regenerate last; workspace tests find stragglers |
| The dispatcher/jounral crash window is mis-modelled | Medium | Duplicate or lost durable history | The journal append is idempotent by event id and stream position; the fault test asserts re-append and single publication |
| Live-bus lag tests become flaky | Medium | Wasted review rounds | Deterministic small capacities, explicit yields, barriers; no sleeps |
| Two-database state drifts | Low | Projection correctness | The dispatcher only reads committed outbox rows and writes the journal; it never touches canonical tables |
| Scope creep into API/transport | Low | Module churn | The port is in-process; transport is API-003's requirement |

## Parallelisation forecast

Fully sequential: each task builds on the previous artifact. The only concurrency inside the
module is in tests.

## Open questions for the user

None blocking. The reconciliation choice (assumption 1) and the journal location (assumption 2)
are decisions I am taking under the standing instruction; they are visible here for correction.

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
