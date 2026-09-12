# Requirements — agentd-command-core

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

## Glossary

| Term | Definition |
|---|---|
| Command envelope | The validated request wrapper: ids, idempotency key, request digest, deadline, command type, and payload bytes. |
| Command type | The fully-qualified protobuf message name of the payload; the registry key. |
| Command handler | The typed implementor that decodes a payload and applies mutations through the transaction. |
| Command context | The authenticated and correlation fields passed to a handler; never the store. |
| Outcome | The success code plus payload bytes recorded for replay and returned to the caller. |
| Replay | Re-execution of an identical command whose idempotency record exists. |
| Fault point | A named coordinator boundary that a test can arm to force a failure deterministically. |
| Fenced transaction | A write transaction that asserts the current daemon fencing epoch at begin. |

## Requirement R1: One idempotent execution path

**User story:** As a kernel engineer, I want every mutation to flow through one coordinator with
replay safety, so that retries and duplicate submissions cannot double-apply commands.

**Addresses:** CMD-001, transaction recipe A, exit criterion "same key/different digest rejected".

**Acceptance criteria (EARS):**

1. WHEN a command envelope is submitted THE SYSTEM SHALL execute it through the single coordinator path and SHALL return exactly the committed outcome.
2. WHEN the same principal, idempotency key, and request digest are submitted again THE SYSTEM SHALL return the stored outcome without repeating any mutation.
3. IF the same principal and key arrive with a different request digest THEN THE SYSTEM SHALL reject with a conflict and SHALL NOT mutate state.
4. IF the command type has no registered handler THEN THE SYSTEM SHALL reject with `InvalidArgument` naming the type and SHALL NOT open a write transaction.
5. WHEN a command completes successfully THE SYSTEM SHALL record the outcome and every staged outbox row in the same transaction as the mutation.
6. WHEN a query or read path needs data THE SYSTEM SHALL NOT route it through the command path.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| fresh command | executed, outcome recorded, row counts increase |
| identical replay | stored outcome returned, row counts unchanged |
| same key, different digest | conflict, no rows changed |
| unknown command type | rejected before any transaction |
| concurrent identical submissions | one mutation commits; the other replays or conflicts |

**Non-goals for R1:** command semantics of the 18 real commands; those arrive in later modules.

---

## Requirement R2: Envelope validation and fencing

**User story:** As an operator, I want malformed or stale commands rejected before any mutation,
so that the store only sees validated, currently fenced work.

**Addresses:** CMD-001 steps 1-3, command-coordinator.md:28-30.

**Acceptance criteria (EARS):**

1. WHEN an envelope is submitted THE SYSTEM SHALL validate that the idempotency key is non-empty and bounded, the request digest is 64 lowercase hexadecimal characters, the command type is non-empty, and the principal and actor ids are present.
2. IF validation fails THEN THE SYSTEM SHALL reject with `InvalidArgument` and SHALL NOT open a transaction.
3. WHEN the envelope carries a deadline in the past THE SYSTEM SHALL reject with `FailedPrecondition` and SHALL NOT mutate.
4. WHEN a write transaction begins THE SYSTEM SHALL assert the current daemon fencing epoch; IF stale or absent THEN THE SYSTEM SHALL reject with `FailedPrecondition` and zero rows changed.
5. WHEN the coordinator opens a transaction THE SYSTEM SHALL use the fencing epoch from the supplied fence provider, never a caller-supplied epoch field.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty idempotency key | `InvalidArgument`, no transaction |
| malformed digest | `InvalidArgument`, no transaction |
| past deadline | `FailedPrecondition`, no transaction |
| stale fence | `FailedPrecondition`, zero rows |
| absent fence | `FailedPrecondition`, zero rows |

**Non-goals for R2:** digest recomputation; clients own the digest algorithm.

---

## Requirement R3: Handlers cannot bypass the transaction

**User story:** As a security reviewer, I want handlers to receive only a transaction and
context, so that no state-changing path can reach a raw store or escape the fenced transaction.

**Addresses:** CMD-001 acceptance, port design from persistence.

**Acceptance criteria (EARS):**

1. WHEN a handler is invoked THE SYSTEM SHALL pass the command context and a mutable transaction reference, and SHALL NOT pass the store or any connection.
2. WHEN handlers are registered THE SYSTEM SHALL key them by command type string and SHALL reject duplicate registrations deterministically.
3. WHERE a test substitutes handlers THE SYSTEM SHALL compile against the same trait object used in production.
4. WHEN a handler stages events THE SYSTEM SHALL provide that ability only through the transaction's stream repository or the events staging API.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| duplicate registration | deterministic rejection at registration |
| handler returns an error | transaction rolls back, no outcome recorded |
| handler stages events then fails | no outbox rows, no mutations |
| handler type not object safe | not representable; trait objects are the production shape |

**Non-goals for R3:** handler authorization policy; later modules enforce capability checks
inside their handlers.

---

## Requirement R4: Crash semantics are deterministic and atomic

**User story:** As a reliability engineer, I want injected failures before and after commit to
produce exactly the pack-required behaviours, so that replay safety is proven, not assumed.

**Addresses:** CMD-001 steps 4, required tests.

**Acceptance criteria (EARS):**

1. WHEN a failure is injected at the pre-commit fault point THE SYSTEM SHALL roll back, leaving zero canonical rows, zero outbox rows, and zero idempotency rows.
2. WHEN a failure is injected at the post-commit fault point THE SYSTEM SHALL return an error after the commit succeeded, and a subsequent identical replay SHALL return the stored outcome with unchanged row counts.
3. WHEN a fault point is armed THE SYSTEM SHALL fire it exactly once per arm and SHALL otherwise behave as a no-op.
4. WHEN no fault is injected THE SYSTEM SHALL never trigger a fault point.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| pre-commit fault | zero rows of any kind |
| post-commit fault | committed rows; error returned; replay returns the outcome |
| replayed fault command | no second mutation, no second outbox row |
| unarmed points | no-op |

**Non-goals for R4:** process-level crash tests; those arrive with the verification module.

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | Envelope payloads and secrets never appear in error messages or logs; validation rejects malformed input before any transaction. |
| N2 | Testability | Every fault point is deterministic and once-per-arm; no test uses a wall-clock sleep. |
| N3 | Compatibility | No contract, schema, or build-pack file changes; foundation and persistence quality gates stay green. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any principal, key, and digest replayed any number of times, the stored outcome is returned and canonical row counts are unchanged. | R1.2 |
| P2 | For any command that fails before commit, no row attributable to it exists. | R4.1 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN the workspace quality gates run THE SYSTEM SHALL CONTINUE TO pass fmt, clippy with warnings denied, and the full test suite. |
| G2 | WHEN the pack validators run THE SYSTEM SHALL CONTINUE TO report `BUILD PACK OK` and `OK` with the contract lock and schema unchanged. |

## Requirements self-analysis

- [x] **Contradictions** — R1's success-only recording and R4's post-commit replay path agree: the outcome and idempotency record commit together in the same transaction, the post-commit fault loses only the response, and the replay finds the stored outcome without re-executing.
- [x] **Ambiguity** — every validation rule states an exact predicate and error code
- [x] **Conflicts** — N3 versus the fault-seam change is resolved: the change is in `agent-os/` code, not in any contract or schema
- [x] **Unstated assumptions** — all referenced concepts are in the Glossary or the plan's assumptions
- [x] **Missing edge cases** — each requirement carries boundary rows including failure and concurrency
- [x] **Testability** — every EARS line maps to a named integration test
- [x] **Coverage** — every CMD-001 step and module-plan in-scope item maps to R1–R4

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| "Fault after commit => replay returns stored outcome" requires the post-commit failure to happen after the outcome is recorded but before the caller sees it | R4.2 | Proposed: the fault point fires after `commit()` returns; the coordinator responds with `Unavailable` (retry-safe); a later identical replay finds the stored outcome. The test asserts the row counts between the two calls. |
| Unknown command type could either open a transaction first or reject before it | R1.4 | Proposed: reject before any transaction; cheaper and leaves no trace. |
| Duplicate handler registration could overwrite silently | R3.2 | Proposed: registration returns an error or panics in tests; overwriting is forbidden. |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
