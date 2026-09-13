# Requirements — agentd-runtime-graph

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

## Glossary

| Term | Definition |
|---|---|
| Runtime entity | Session, AgentSpec revision, Task, or AgentRun persisted in the kernel store. |
| Run state machine | The normative transition table from `specs/runtime-manager.md`. |
| Recovery disposition | A classification separate from run state describing restart handling. |
| Dependency edge | A `run_dependencies` row declaring that a target run waits on a source condition. |
| Readiness | The state where a `Ready` run's dependencies and recovery allow claiming. |
| Claim | A fenced, expiring ownership of a ready run transitioned to `Running`. |
| Cancellation epoch | A per-run counter advanced on cancellation; child creation must observe it. |
| AgentSpec revision | An immutable `(id, version)` body with a digest; never overwritten. |
| Graph head | The `run_graph_heads` row per task whose revision advances with edge mutations. |

## Requirement R1: Runtime entity creation is atomic and immutable where required

**User story:** As a kernel engineer, I want sessions, agent specs, tasks, and created runs to be
persisted atomically through the coordinator, so that every later runtime decision has complete,
auditable roots.

**Addresses:** RUN-000, recipes B and D.

**Acceptance criteria (EARS):**

1. WHEN a session is created THE SYSTEM SHALL persist it bound to the authenticated principal and stage `SessionCreated` in the same transaction.
2. WHEN an AgentSpec revision is stored THE SYSTEM SHALL insert it immutably; an identical `(id, version)` with the same digest SHALL be idempotent, and different bytes SHALL conflict.
3. WHEN a task is created THE SYSTEM SHALL insert its `run_graph_heads` row in the same transaction.
4. WHEN `CreateTaskRun` executes THE SYSTEM SHALL atomically create the task (when new) and the run in `Created`, persist parent linkage through `runs.parent_run_id`, stage the canonical events, and record the idempotency outcome.
5. IF a parent run is present THEN THE SYSTEM SHALL require the request's observed parent cancellation epoch to equal the persisted epoch, and SHALL reject the child otherwise.
6. WHEN a run is created THE SYSTEM SHALL NOT transition it to `Ready`; only a `BindRun` with a persisted resolved environment may do so.
7. WHEN any of these commands completes THE SYSTEM SHALL have no canonical row without its outbox events and idempotency record.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| duplicate session id with same request | replay returns the stored outcome |
| same AgentSpec revision, same digest | idempotent success |
| same AgentSpec revision, different bytes | conflict, first revision untouched |
| child with absent or stale observed epoch | rejected, no child row |
| run creation | state `Created`, never `Ready` |

**Non-goals for R1:** environment resolution and `BindRun` (config module).

---

## Requirement R2: Run transitions are table-driven and revisioned

**User story:** As a reliability engineer, I want every run mutation to follow the normative table
and increment the revision, so that stale loop decisions and races are detectable.

**Addresses:** RUN-001, `specs/runtime-manager.md`.

**Acceptance criteria (EARS):**

1. WHEN a run transition is requested THE SYSTEM SHALL allow it only if the transition table permits it from the current state.
2. IF the transition is illegal THEN THE SYSTEM SHALL reject with a conflict and leave no mutation.
3. WHEN an authoritative run mutation commits THE SYSTEM SHALL increment `run_revision` exactly once.
4. WHEN a run reaches a terminal state THE SYSTEM SHALL persist a stable terminal reason and SHALL reject further normal transitions.
5. WHEN recovery changes only the recovery disposition THE SYSTEM SHALL persist it independently of run state without inventing an illegal transition.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| Created to Running | rejected (only Created to Ready) |
| Running to Completed | accepted, terminal reason stored |
| terminal to anything | rejected |
| revision race | CAS mismatch rejects the stale writer |
| recovery disposition update | state unchanged, disposition persisted |

**Non-goals for R2:** loop decision fencing (loop module).

---

## Requirement R3: Dependency edges are transactional and cycle-free

**User story:** As a runtime engineer, I want dependency edges that cannot form cycles or mutate
frozen targets, so that readiness is always decidable.

**Addresses:** RUN-002, `specs/run-graph.md`.

**Acceptance criteria (EARS):**

1. WHEN an edge is added THE SYSTEM SHALL require source and target to belong to the same task graph and the target to be `Created` or unclaimed `Ready`.
2. WHEN inserting the edge THE SYSTEM SHALL reject it if the target already reaches the source, so no committed dependency graph contains a directed cycle.
3. WHEN an edge is inserted THE SYSTEM SHALL increment the task's graph revision in the same transaction and stage `DependencyAdded`.
4. IF the same edge already exists THEN THE SYSTEM SHALL treat it as an idempotent duplicate or a conflict without a second row.
5. WHEN two writers race opposite edges THE SYSTEM SHALL commit at most one of the cycle-forming pair.
6. WHEN parent-child relationships are needed THE SYSTEM SHALL derive them from `runs.parent_run_id` and SHALL NOT create a second ancestry table.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| self-edge | rejected |
| cross-task edge | rejected |
| target running or terminal | rejected |
| duplicate edge | one row, deterministic outcome |
| opposite-edge race | acyclic result |

**Non-goals for R3:** workflow planning semantics.

---

## Requirement R4: Readiness and claims have exactly one winner

**User story:** As an operator, I want claims that honor dependencies and recovery, so that a run
starts once and only when it may.

**Addresses:** RUN-003, recipe F.

**Acceptance criteria (EARS):**

1. WHEN a dependency condition is evaluated THE SYSTEM SHALL implement exactly `completed_successfully`, `any_terminal`, and `completed_or_cancelled`.
2. WHEN a `Ready` run is claimed THE SYSTEM SHALL verify dependencies, recovery disposition `Normal`, no unexpired claim, and no cancellation block inside the claim transaction.
3. WHEN the claim commits THE SYSTEM SHALL transition the run to `Running` with owner, token, expiry, and issuing daemon epoch, incrementing the revision.
4. IF 100 writers race one claim THEN THE SYSTEM SHALL commit exactly one.
5. WHEN a claim is expired THE SYSTEM SHALL allow reclamation only for runs whose recovery disposition permits it.
6. IF a run is blocked by recovery or cancellation THEN THE SYSTEM SHALL reject claiming it without mutation.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| unmet dependency | claim rejected |
| completed_or_cancelled source succeeded | satisfied |
| any_terminal source failed | satisfied |
| live unexpired claim | claim rejected |
| expired claim, recovery Normal | reclaimable |
| blocked disposition | rejected |

**Non-goals for R4:** resource revalidation; resources module later.

---

## Requirement R5: Cancellation epochs fence child creation

**User story:** As a reliability engineer, I want cancellation to win every spawn race, so that no
child escapes a cancelled parent.

**Addresses:** RUN-004, recipe G.

**Acceptance criteria (EARS):**

1. WHEN a run is cancelled THE SYSTEM SHALL increment its cancellation epoch inside the transaction.
2. WHEN the epoch advances THE SYSTEM SHALL transition the root and all known non-terminal descendants toward `Cancelling` (or `Cancelled` when they never ran), leaving terminal descendants unchanged.
3. WHEN a child creation races cancellation THE SYSTEM SHALL reject any spawn whose observed epoch does not match the committed epoch.
4. WHEN cancellation propagation commits THE SYSTEM SHALL stage cancellation events and increment affected revisions.
5. WHEN a descendant is already terminal or `Cancelling` THE SYSTEM SHALL leave it unchanged.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| cancel/spawn barrier race | spawn rejected after epoch advance |
| nested descendants | all eligible runs transition |
| terminal child | unchanged |
| duplicate cancel | epoch advances again, terminal runs untouched |

**Non-goals for R5:** workspace revocation and effect compensation.

---

## Requirement R6: Startup reconstruction never guesses

**User story:** As an operator, I want restart classification that fails closed, so that an
ambiguous external effect is never blindly retried.

**Addresses:** RUN-005, `specs/recovery-table.md`, exit criterion 11.

**Acceptance criteria (EARS):**

1. WHEN the daemon starts THE SYSTEM SHALL enumerate non-terminal runs and load their frozen environments, effects, and timers.
2. WHEN classifying a run THE SYSTEM SHALL assign exactly one disposition per the normative recovery matrix, without changing run state.
3. IF a reachable combination has no matrix rule THEN startup SHALL fail closed rather than choose a default.
4. WHEN an effect outcome is ambiguous THE SYSTEM SHALL persist `BlockedUnknownEffect` or `NeedsReconciliation` as the matrix dictates and SHALL NOT mark the run resumable.
5. WHEN dispositions are persisted THE SYSTEM SHALL record them durably with the audit identifiers needed later.
6. WHEN a run's disposition is not `Normal` THE SYSTEM SHALL NOT issue loop turns for it.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| Running run, no effects | `Recovering` or `Normal` per matrix |
| unknown effect | blocked disposition |
| missing adapter binding | `BlockedMissingResource` |
| unmapped combination | startup fails closed |
| terminal runs | not enumerated |

**Non-goals for R6:** automatic reconciliation execution (effects module).

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | Handlers receive context and transaction only; payload bytes never appear in errors or logs; no raw SQL outside the SQLite crate. |
| N2 | Testability | Races use barriers and fault points; no wall-clock sleeps in tests; property tests cover transition sequences and cycle-freedom. |
| N3 | Compatibility | No contract or schema changes; the three port additions keep the mock faithful; all gates and validators stay green. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any interleaving of edge insertions, the committed dependency graph is acyclic. | R3.2, R3.5 |
| P2 | For any set of racing claimants on one ready run, exactly one claim commits. | R4.4 |
| P3 | For any cancel/spawn race, no child commits with a stale observed epoch. | R5.3 |
| P4 | For any restart state, a run with an ambiguous effect is never left resumable. | R6.2, R6.4 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN the workspace quality gates run THE SYSTEM SHALL CONTINUE TO pass fmt, clippy with warnings denied, and the full suite. |
| G2 | WHEN the pack validators run THE SYSTEM SHALL CONTINUE TO report `BUILD PACK OK` and `OK` with contracts and schema unchanged. |

## Requirements self-analysis

- [x] **Contradictions** — R1.6's "never Ready without BindRun" and R4's claims are consistent: claims require `Ready`, which only `BindRun` produces
- [x] **Ambiguity** — every condition and disposition maps to a named enum or matrix row
- [x] **Conflicts** — R5's epoch check and R1.5's child rule are the same mechanism stated from both sides; implemented once
- [x] **Unstated assumptions** — port additions and in-memory ancestry walks are in the plan's assumptions
- [x] **Missing edge cases** — each requirement carries race, terminal, and duplicate boundaries
- [x] **Testability** — barrier races, table-driven transitions, property suites; no sleeps
- [x] **Coverage** — every RUN brief item maps to R1-R6

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| `Ready` cannot be reached in this module, so claim tests need a way to produce it | R4 | Tests set up `Ready` runs through a direct repository write inside a test transaction (not a production path), documented as test scaffolding |
| Recovery needs to enumerate runs and their effects/timers, but the port has no list-by-run methods | R6 | Planned port additions (`RunRead::list_active`, `EffectRead::list_by_run`, `TimerRead::list_by_run`) with SQLite and mock implementations |
| Cancellation target states for never-started runs | R5.2 | Proposed: `Created`/`Ready` move directly to `Cancelled`; started states move to `Cancelling` |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
