# Requirements — agentd-persistence

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

This module spec covers tasks PST-001 through PST-005 of the approved umbrella plan
(`.spec/agentd-microkernel-mvp/plan.md`). Requirements are behavioural; the schemas, trait
shapes, and transaction recipes they reference are normative in the build pack.

## Glossary

| Term | Definition |
|---|---|
| KernelStore | The trusted persistence port: begins read/write transactions and acquires the daemon fence. |
| KernelTxn | A write transaction object exposing typed repository operations and consuming commit/rollback. |
| KernelReadTxn | A read-only view that cannot mutate canonical state. |
| Inception schema | The single initial SQLite schema, executed verbatim once. The MVP has no migrations. |
| Schema version | The value seeded in `kernel_meta` at bootstrap; anything but the inception version fails closed. |
| Daemon fencing epoch | A durable counter claimed once per daemon instance; every write transaction asserts it. |
| Writer ordering | Reserving the SQLite write lock before validating a read-modify-write sequence (BEGIN IMMEDIATE). |
| CAS | Compare-and-set: mutate only when stored revision/state/version/epoch matches the expectation. |
| Idempotency record | The stored outcome for a principal plus idempotency key, bound to the request digest. |
| Stream head | The persisted last sequence per event stream; allocation is contiguous and transactional. |
| Outbox row | An immutable staged event with a unique event id and stream position, awaiting journal publication. |
| Repository | A typed operation group over one entity family, reachable only through a transaction object. |
| Runtime root | The directory holding `kernel.db` and the daemon lock, resolved from the module plan's `AGENTD_HOME` decision. |

## Requirement R1: Versioned bootstrap of the kernel database

**User story:** As a kernel engineer, I want a database that bootstraps exactly once from the
inception schema and refuses unknown versions, so that no component can invent structure at
runtime.

**Addresses:** PST-001, invariant "only the exact inception schema is accepted".

**Acceptance criteria (EARS):**

1. WHEN the kernel database does not exist THE SYSTEM SHALL create it from the inception schema verbatim, seed the schema version to 1, and apply the normative PRAGMA set and file modes.
2. WHEN the kernel database exists with the inception schema version THE SYSTEM SHALL open it without creating or altering any table.
3. IF the recorded schema version is absent or different from 1 THEN startup SHALL fail closed and name the version found.
4. WHEN repositories initialize THE SYSTEM SHALL NOT create, alter, or drop tables outside the bootstrap step.
5. WHEN bootstrap completes THE SYSTEM SHALL have foreign keys enforced, write-ahead journaling, the normative synchronous level and busy timeout, the database file mode `0600`, and its directory mode `0700`.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| fresh directory | database created, version seeded, modes applied |
| existing version 1 database | opened read/write with no structural change |
| version 0 or 2 or missing | startup fails closed naming the value |
| corrupt or unreadable file | startup fails closed, no partial rewrite |
| second open while first holds the file | open succeeds or fails per SQLite locking, never silently forks state |

**Non-goals for R1:** migrations, schema upgrades, or in-place repair.

---

## Requirement R2: Object-safe transaction contract

**User story:** As the command coordinator author, I want one transaction contract with typed
operations and no SQL escape hatch, so that canonical state can only change through the intended
path.

**Addresses:** PST-002, decision D-028 (one process, library crates).

**Acceptance criteria (EARS):**

1. WHEN a caller begins a write transaction THE SYSTEM SHALL return an object-safe transaction carrying the expected daemon fencing epoch and exposing typed repository groups only.
2. WHEN a caller begins a read transaction THE SYSTEM SHALL return a view that exposes reads and SHALL NOT expose mutation operations.
3. WHEN a write transaction commits THE SYSTEM SHALL persist every staged mutation atomically or none of them.
4. IF a transaction is dropped without commit THEN THE SYSTEM SHALL release the connection and leave no partial canonical rows.
5. WHEN kernel code outside the SQLite implementation needs data access THE SYSTEM SHALL NOT expose arbitrary SQL execution.
6. WHERE a test substitutes a mock store implementation THE SYSTEM SHALL compile and run the same trait objects used by production code.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty transaction | commit is a no-op success; drop performs no write |
| commit after rollback or double commit | rejected deterministically, never a second commit |
| transaction dropped mid-work | automatic rollback, connection returned to the pool |
| mock versus SQLite implementation | identical trait signatures, no SQLite types in the interface |
| concurrent transactions from one store | each gets its own connection and isolation |

**Non-goals for R2:** command semantics, retries, or scheduling; those belong to the command
coordinator.

---

## Requirement R3: Typed repositories with storage-enforced correctness

**User story:** As a kernel engineer, I want repositories that validate stored values and apply
CAS semantics through database conditions, so that racing writers cannot corrupt state.

**Addresses:** PST-003, invariants "every correctness constraint is backed by DB condition".

**Acceptance criteria (EARS):**

1. WHEN a repository reads a persisted enum or state value THE SYSTEM SHALL decode it through the domain mirror enums and fail closed on any unknown value.
2. WHEN a mutation expects a specific revision, state, version, or fencing epoch THE SYSTEM SHALL mutate only if the stored value matches, and SHALL report a conflict otherwise.
3. WHEN a mutation reserves writer ordering for a read-modify-write sequence (graph heads, stream heads, claims) THE SYSTEM SHALL acquire the SQLite write lock before validation.
4. IF a storage constraint rejects a write THEN THE SYSTEM SHALL return a stable machine-readable error code, distinguishing conflict, failed precondition, busy, and internal invariant breaches.
5. WHEN any write transaction fails THE SYSTEM SHALL leave zero partial canonical rows.
6. WHEN a resolved run environment or another immutable record is targeted by an update THE SYSTEM SHALL have that update rejected by the storage layer.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| unknown persisted enum | read fails closed with an internal invariant error |
| stale expected revision | CAS reports conflict, no rows changed |
| two writers racing one CAS | exactly one succeeds |
| write lock contended | bounded wait, then a busy error with retry-safe classification |
| constraint violation mid-transaction | rollback leaves zero partial rows |

**Non-goals for R3:** domain decisions about which transition is legal; repositories enforce
expectations supplied by callers.

---

## Requirement R4: One authoritative daemon with a durable fence

**User story:** As an operator, I want exactly one writer daemon with a persisted epoch, so that
a stale process can never commit authoritative work.

**Addresses:** PST-004, decisions D-001 and D-004.

**Acceptance criteria (EARS):**

1. WHEN `agentd` starts THE SYSTEM SHALL acquire an exclusive OS lock on the runtime lock file before serving any authoritative work, and IF the lock is held THEN startup SHALL refuse to become authoritative.
2. WHEN a daemon instance acquires the lock THE SYSTEM SHALL claim a durable fencing epoch strictly greater than every previously persisted epoch.
3. WHEN a write transaction executes THE SYSTEM SHALL assert that its epoch equals the current persisted epoch, and IF stale THEN SHALL reject the transaction without mutation.
4. WHEN the daemon loses the lock or its fence is superseded THE SYSTEM SHALL refuse new authoritative work and transition to draining.
5. WHEN the daemon restarts THE SYSTEM SHALL increment the persisted epoch, so pre-restart in-flight work is fenced out.
6. WHEN a second daemon process attempts to start against the same runtime root THE SYSTEM SHALL leave the database untouched by the second process.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| first process starts | lock acquired, epoch claimed |
| second process starts | refuses, first process unaffected |
| first process killed without cleanup | OS releases the lock; next start increments the epoch |
| write with stale epoch | rejected, zero rows changed |
| fence row missing at bootstrap | created on first claim |

**Non-goals for R4:** leader election across machines, or a remote lock service.

---

## Requirement R5: Replay-safe idempotency

**User story:** As a client author, I want replays to return the stored outcome and conflicts to
be explicit, so that retries never double-apply a command.

**Addresses:** PST-005, exit criterion "same key/different digest is a conflict".

**Acceptance criteria (EARS):**

1. WHEN a command transaction runs THE SYSTEM SHALL look up the idempotency record by principal and key within that same transaction.
2. WHEN the same principal, key, and request digest recur THE SYSTEM SHALL return the stored outcome and SHALL NOT repeat any mutation.
3. IF the same principal and key arrive with a different request digest THEN THE SYSTEM SHALL reject with a conflict and SHALL NOT mutate state.
4. WHEN an outcome is recorded THE SYSTEM SHALL store the exact outcome code and payload in the command transaction, so the record and effects commit together.
5. IF two identical submissions race THEN THE SYSTEM SHALL commit exactly one mutation and serve the other from the stored outcome or a conflict.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| fresh key | record inserted with the outcome |
| same key, same digest | stored outcome returned, no mutation |
| same key, different digest | conflict, no mutation |
| concurrent identical submissions | one insert wins, the loser reads or conflicts |
| empty or oversized key | rejected as invalid input before mutation |

**Non-goals for R5:** request digest computation itself; the digest algorithm is normative in the
foundation spec.

---

## Requirement R6: Transactional outbox with contiguous stream sequences

**User story:** As an events consumer, I want every staged event to have a durable, gap-free
stream position committed with its cause, so that publication can be retried safely.

**Addresses:** PST-005, decision D-005 (journal downstream, outbox first).

**Acceptance criteria (EARS):**

1. WHEN a durable event is staged THE SYSTEM SHALL allocate the next contiguous sequence for its stream by updating the persisted stream head within the same transaction as the mutation that caused it.
2. WHEN an outbox row is inserted THE SYSTEM SHALL enforce a unique event id and a unique stream position through database constraints.
3. WHEN the outbox is scanned for publication THE SYSTEM SHALL return unpublished rows ordered by stream and sequence and SHALL NOT modify canonical event content.
4. IF two concurrent transactions stage events on the same stream THEN THE SYSTEM SHALL allocate distinct contiguous sequences with no gaps and no duplicates.
5. WHEN a mutation commits THE SYSTEM SHALL have its outbox rows visible with the same commit; WHEN it fails THE SYSTEM SHALL have none.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| first event on a stream | sequence 1 at the stream head |
| duplicate event id | storage conflict, transaction rolls back |
| gap attempt (sequence skip) | rejected by the stream-head update condition |
| concurrent appends | contiguous distinct positions, one winner per position |
| scan with no unpublished rows | empty ordered result |

**Non-goals for R6:** journal publication, live subscription, or event payload classification
rules; those are the events module's requirements.

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | `kernel.db` is created `0600` in a `0700` directory; no SQL escape hatch is exposed outside the SQLite crate; errors and logs contain no secret or payload material. |
| N2 | Testability | Every concurrency and crash boundary in this module is exercised through deterministic barriers and testkit fault points; no test synchronizes with a wall-clock sleep. |
| N3 | Compatibility | This module changes no contract file, schema file, or build-pack validator; the foundation quality gates and both pack validators stay green. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any write transaction that fails or is dropped, the store contains zero rows attributable to it. | R2.4, R3.5 |
| P2 | For any set of writers racing one CAS expectation, exactly one commits. | R3.2 |
| P3 | For any interleaving of concurrent appends to one stream, allocated sequences are unique and contiguous. | R6.4 |
| P4 | For any replay with the same principal, key, and digest, the stored outcome is returned and the row counts of canonical tables are unchanged. | R5.2 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN the foundation quality gates run THE SYSTEM SHALL CONTINUE TO pass fmt, clippy with warnings denied, and the full workspace test suite. |
| G2 | WHEN the pack validators run THE SYSTEM SHALL CONTINUE TO report `BUILD PACK OK` and the repo validation `OK` with the contract lock and schema unchanged. |

## Requirements self-analysis

- [x] **Contradictions** — checked against the module plan and pack recipes; R4's fence and R2's epoch carry are complementary, not conflicting
- [x] **Ambiguity** — every state, version, and epoch reference maps to a named schema column or domain type
- [x] **Conflicts** — R2's no-SQL rule and R3's CAS SQL are resolved by keeping SQL private to the SQLite crate
- [x] **Unstated assumptions** — every referenced concept is in the Glossary or the plan's assumptions
- [x] **Missing edge cases** — each requirement carries boundary rows including failure and concurrency
- [x] **Testability** — every EARS line maps to a named test the pack briefs require
- [x] **Coverage** — every PST brief item and module-plan in-scope item maps to R1–R6

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| R3.4 promises distinguishable busy and conflict codes, but the foundation error enum has no dedicated store code | R3.4, N3 | Proposed: map busy/timeout to `Unavailable` with retry-safe classification and constraint breaches to `Conflict` or `FailedPrecondition`; no error-model change in this module. Confirm |
| The daemon "draining" state has no schema column in the inception schema | R4.4 | Proposed: draining is an in-memory daemon state that refuses new transactions; persistence is unnecessary because the fence itself blocks stale work. Confirm |
| Ambiguous key validation limits are not stated in the pack | R5 boundary row | Proposed: reject empty keys and keys over 255 bytes with `InvalidArgument`, matching `IdempotencyKey` from the foundation. Confirm |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (recorded from explicit chat instruction)
**Date:** 2026-09-11
