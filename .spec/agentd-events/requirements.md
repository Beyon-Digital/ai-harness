# Requirements — agentd-events

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

## Glossary

| Term | Definition |
|---|---|
| Durable event | An event with id, type, version, stream key, sequence, classification, and occurred time, safe for history. |
| Stream key | The canonical string identifying an event stream (`run/...`, `task/...`, `session/...`, `effect/...`, `config/global`, `adapter/...`, `security/principal/...`). |
| Cursor | The opaque `v1:{stream_key}:{sequence}` token addressing a position in a stream. |
| Expected sequence | The stream head the appender believes precedes its first event; a mismatch is a gap. |
| Event Journal | The downstream `events.db` projection with exact append rules; not an authority for canonical state. |
| Dispatcher | The worker that projects committed outbox rows into the journal and then to the live bus. |
| Publication mark | The outbox metadata column recording journal or live acceptance. |
| Lag | A live subscriber falling behind the bounded buffer; reported with a resume cursor, never silently skipped. |
| Ephemeral telemetry | Non-durable lossy signals delivered on a separate channel. |

## Requirement R1: Validated durable event primitives

**User story:** As a kernel engineer, I want one validated event shape with canonical stream
keys and cursors, so that durable history has no untyped or unclassifiable records.

**Addresses:** EVT-001, foundation R9.4, the deferred vocabulary reconciliation.

**Acceptance criteria (EARS):**

1. WHEN a durable event is built THE SYSTEM SHALL require an event id, event type, event version, stream key, sequence, classification, and occurred time, and SHALL refuse construction when any is absent.
2. WHEN a stream key is constructed THE SYSTEM SHALL use exactly the canonical forms in the event pipeline spec, and every constructed key SHALL round-trip through parse and display.
3. WHEN a cursor is built or parsed THE SYSTEM SHALL use the canonical `v1:{stream_key}:{sequence}` form; IF the form is malformed THEN the parse SHALL fail closed.
4. WHEN a classification is assigned to an event THE SYSTEM SHALL accept it only if it is at or above the event type's declared minimum; downgrades SHALL be rejected.
5. THE SYSTEM SHALL spell every classification value identically in the contract proto, the domain mirror enums, the event catalog, and the schema comments, with sensitivity 1-4 and retention 1-3.
6. WHEN an event envelope is encoded and decoded THE SYSTEM SHALL preserve every field exactly.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| missing classification | construction refused |
| unknown stream prefix | parse refused |
| malformed cursor (`v1::3`, `v2:x:1`, trailing text) | parse refused |
| downgrade below the type minimum | rejected |
| envelope round trip | fields identical, payload bytes preserved |

**Non-goals for R1:** event type registry contents; the catalog is normative and unchanged.

---

## Requirement R2: Exact journal append semantics

**User story:** As an integrator, I want the journal to accept each stream position exactly
once, so that consumers can trust sequence numbers.

**Addresses:** EVT-002, decision D-005.

**Acceptance criteria (EARS):**

1. WHEN events are appended THE SYSTEM SHALL require the expected sequence to equal the current stream head and SHALL reject gaps.
2. WHEN an identical event id already exists at the requested stream position THE SYSTEM SHALL return idempotent success without writing.
3. IF a different event id occupies the requested stream position THEN THE SYSTEM SHALL reject as a conflict.
4. WHEN a stream is read THE SYSTEM SHALL return events after the requested sequence up to the limit, in sequence order, and SHALL report the retention-gap flag (false in the inception implementation).
5. WHEN the journal is written THE SYSTEM SHALL NOT modify any canonical kernel table, and the journal SHALL live in a separate database file.
6. IF two appends race for the same stream position THEN THE SYSTEM SHALL commit exactly one and reject the other.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| first append with expected 0 | accepted, head advances |
| append with expected behind head | gap rejection |
| exact duplicate | idempotent success, no new row |
| same position, different id | conflict |
| read past the head | empty result, no error |
| concurrent same-position appends | one winner |

**Non-goals for R2:** journal deletion, compaction, or retention enforcement.

---

## Requirement R3: Journal-first dispatch survives crashes

**User story:** As a reliability engineer, I want the outbox projected durably before any live
delivery, so that no subscriber can observe an event the journal does not have.

**Addresses:** EVT-003, exit criterion "restarted publisher re-publishes safely".

**Acceptance criteria (EARS):**

1. WHEN the dispatcher runs THE SYSTEM SHALL scan unpublished outbox rows ordered by stream key and sequence.
2. WHEN a row is appended successfully THE SYSTEM SHALL mark journal publication; IF the journal append fails THEN no mark SHALL be written and the row SHALL remain unpublished.
3. WHEN a process crash occurs after a journal append but before the publication mark THE SYSTEM SHALL re-append idempotently after restart and SHALL produce no duplicate journal row.
4. WHEN the journal is unavailable THE SYSTEM SHALL retry with capped backoff while commands continue to commit and the unpublished backlog grows.
5. WHEN several rows for one stream are pending THE SYSTEM SHALL publish them in sequence order.
6. WHEN journal publication succeeds THE SYSTEM SHALL hand the event to the live bus only then; no live delivery SHALL precede journal acceptance.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty outbox | no-op iteration |
| crash after append, before mark | identical re-append, one row |
| journal unavailable | backlog grows, commands unaffected |
| multi-stream backlog | per-stream order preserved |
| duplicate dispatcher iterations | no double publication |

**Non-goals for R3:** multi-process dispatch coordination (one daemon owns the store).

---

## Requirement R4: Bounded live bus with explicit lag

**User story:** As a client author, I want slow subscribers disconnected with a resumable
cursor, so that I can catch up from durable history instead of losing events.

**Addresses:** EVT-004, exit criterion "a live durable subscriber never receives a resumable cursor before the Event Journal contains that event".

**Acceptance criteria (EARS):**

1. WHEN a durable event is published THE SYSTEM SHALL deliver it to each live subscriber in stream order.
2. WHEN a subscriber falls behind beyond the buffer capacity THE SYSTEM SHALL return an explicit lag result carrying the last journal-backed cursor and SHALL terminate that subscription.
3. WHEN a subscriber resumes from a lag result THE SYSTEM SHALL allow reading the journal from that cursor with no gap and no duplicate.
4. WHEN ephemeral telemetry is published THE SYSTEM SHALL use a separate lossy channel that can drop without affecting durable delivery or cursors.
5. THE SYSTEM SHALL NOT advance a durable cursor for an event that was not delivered, and SHALL NOT deliver an event the journal has not accepted.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| subscriber at capacity | lag result with resume cursor |
| resume after lag | journal read continues exactly after the cursor |
| one slow subscriber | other subscribers unaffected |
| ephemeral overflow | drops, no durable impact |
| subscriber disconnect | no state left behind |

**Non-goals for R4:** cross-process fan-out or transport encoding; API-003 owns the wire surface.

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | Classification downgrades are rejected; payload bytes never appear in errors or logs; `events.db` is created `0600` in the `0700` runtime directory. |
| N2 | Testability | Dispatch iterations and lag are driven deterministically without wall-clock sleeps; tests use explicit iteration calls and small capacities. |
| N3 | Compatibility | The only contract and schema changes are the approved vocabulary reconciliation; all other gates stay green and the mirror check stays clean. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any crash point between journal append and publication mark, exactly one journal row exists per event after recovery. | R3.3 |
| P2 | For any interleaving of appends to one stream, journal sequences are contiguous with no gaps or duplicates. | R2.1, R2.6 |
| P3 | For any subscription, no delivered event is absent from the journal. | R3.6, R4.5 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN the workspace quality gates run THE SYSTEM SHALL CONTINUE TO pass fmt, clippy with warnings denied, and the full suite. |
| G2 | WHEN the pack validators and mirror check run THE SYSTEM SHALL CONTINUE TO pass after the reconciliation. |

## Requirements self-analysis

- [x] **Contradictions** — R3's journal-first rule and R4's lag rule are consistent: lag can only reference journal-backed positions
- [x] **Ambiguity** — retention has exactly three values; the CHECK narrows to 1-3; every predicate names the exact form
- [x] **Conflicts** — N3 versus R1.5 is resolved: the reconciliation is the one approved change, performed by EVT-G0 and validated by the pack validators
- [x] **Unstated assumptions** — all terms are in the Glossary or the plan's assumptions
- [x] **Missing edge cases** — each requirement carries boundary rows including crash and concurrency
- [x] **Testability** — deterministic iteration, small capacities, fault injection; no sleeps
- [x] **Coverage** — every EVT brief item and plan in-scope item maps to R1-R4

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| The catalog has three retention classes while the proto and domain have four | R1.5 | Proposed and recorded: retain `EPHEMERAL=1`, rename `SESSION` to `STANDARD=2`, keep `AUDIT=3`, remove `DURABLE`; the outbox CHECK narrows to 1-3 |
| The dispatcher cannot be tested deterministically if its loop always sleeps | R3, N2 | Proposed: expose a single dispatcher iteration; the worker loop owns the interval and backoff |
| Where does the journal file live | R2.5 | Proposed: alongside `kernel.db` in the runtime root, `events.db`, mode `0600` |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
