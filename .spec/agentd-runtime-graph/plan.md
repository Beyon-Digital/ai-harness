# Plan — agentd-runtime-graph

**Status:** draft
**Date:** 2026-09-11

Module spec. Phase 1 input is the approved umbrella plan at
`.spec/agentd-microkernel-mvp/plan.md`. This module is fifth in the build order; foundation,
persistence, command-core, and events are complete and approved. Approvals below are recorded
under the user's standing instruction to proceed without per-step confirmation.

## Problem statement

Nothing can create runtime entities or mutate run state. The persistence module provides the
tables, repositories, and transaction port; the command coordinator provides the single mutation
path; the events module provides durable projection. But there are no command handlers for
sessions, agent specs, tasks, or runs; no run state machine; no dependency-graph service,
readiness evaluation, single-winner claims, cancellation epochs, or startup recovery. Without
them the daemon cannot run anything.

## Outcome

The runtime foundation is real: immutable AgentSpec revisions, session and task creation, and
`CreateTaskRun` producing a `Created` run with parent linkage and no premature `Ready`; a
transition table enforced on every mutation with revision increments; transactional dependency
edges with cycle prevention; deterministic readiness and exactly-one-winner claims; cancellation
epochs that reject stale child spawns and propagate through the known subtree; and startup
reconstruction that assigns recovery dispositions without abusing run state. Tasks RUN-000
through RUN-005 are implemented and reviewed.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | Handler payloads decode the generated prost messages from `domain::generated::contract` (the 18 catalogued commands already have wire schemas) | A different decoding path changes RUN-000 and later handlers |
| 2 | Handlers are registered through a shared registry builder so later modules extend one command surface | Registration shape changes at the control-api module |
| 3 | Descendant and child queries walk `runs.parent_run_id` via `list_by_task` in memory for the MVP; no second ancestry table (spec requirement) | A SQL recursive CTE or a port method replaces the in-memory walk |
| 4 | Port additions are needed and allowed: `RunRead::list_active` for recovery, and `EffectRead`/`TimerRead::list_by_run`; the mock mirrors them | Recovery cannot enumerate what to classify |
| 5 | Cancellation policy for the MVP: `Created`/`Ready`/`Waiting*`/`Suspended` non-terminal runs move to `Cancelling`, and the recovery/terminalization step moves `Cancelling` to `Cancelled`; terminal runs are untouched | A different target state changes the cancellation tests and events |
| 6 | Child creation requires `observed_parent_cancellation_epoch` whenever a parent is present, and rejects on mismatch or absence | Optional observation would weaken the escape guarantee |
| 7 | Recovery classification sources: effects by run, timers by run, and the frozen environment's adapter bindings; ambiguous effects dominate the disposition | Different precedence changes the disposition matrix implementation |
| 8 | `BindRun` and environment resolution stay with the config module (CFG-003); this module only guarantees `Ready` cannot be reached without one | BindRun would move into RUN-001/RUN-003 |

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| The state machine and revision rules are normative | `specs/runtime-manager.md:6-39` | RUN-001 encodes exactly this table; recovery dispositions are separate |
| Dependency conditions, edge rules, claim eligibility, and cancellation epoch are normative | `specs/run-graph.md` (all sections) | RUN-002/003/004 implement these rules; no DSLs |
| Recipes B, D, E, F, G, and the head-row rule fix transaction order | `specs/transaction-recipes.md`; `specs/run-graph.md` D16 | Handlers compose the coordinator transaction; graph heads exist from task creation |
| The persistence port exposes runs, tasks, sessions, graph, effects, timers, environments, and streams | `agent-os/crates/kernel-store/src/repositories.rs`; persistence reports | RUN-000-005 consume the port only; the three list methods are the additions needed |
| The coordinator is the only mutation path and provides fault points | command-core reports; `crates/command-coordinator/src/handler.rs` | Every handler is a `CommandHandler`; tests drive `execute` with envelopes |
| The outbox staging API exists | `crates/events/src/outbox.rs` (`stage`) | Handlers stage canonical events in-transaction |
| The catalog defines the emitted event types | `contracts/events/catalog.yaml` | SessionCreated, AgentSpecRevisionStored, TaskCreated, RunCreated, ChildRunCreated, DependencyAdded, RunClaimed, RunStarted, CancellationEpochAdvanced, run-state events, recovery-disposition events |
| Recovery dispositions and the matrix are normative | `specs/recovery-table.md`; `contracts/domain/core.proto` | RUN-005 classifies per the matrix; unmapped combinations fail closed |
| Tables and CHECK literals exist | `agent-os/schema/kernel_store.sql` | No schema changes needed |

## Existing conventions to follow

- All commands from `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Pack validators and the mirror check from the repo root.
- No `unwrap()`/`expect()` outside tests; no sleeps; no payload bytes in errors; handlers receive
  context and transaction only.
- Commit per task with the task id; reports under this spec's `reports/` directory.

## Approach

### Chosen

Six sequential tasks, one per pack brief, plus the port additions folded into the task that
needs them:

1. **RUN-000** — declare dependencies; add the `agent_specs` repository (insert-only, immutability
   enforced by schema triggers); implement `CreateSession`, `PutAgentSpecRevision`, `CreateTaskRun`
   handlers; task creation inserts the graph head in the same transaction; child creation checks
   the observed parent cancellation epoch; runs start `Created`.
2. **RUN-001** — the transition table with terminal reasons and mandatory revision increments;
   property test over arbitrary transition sequences.
3. **RUN-002** — the `run-graph` dependency service: same-task validation, mutable-target rule,
   reachability rejection, idempotent duplicates, `DependencyAdded` staging, and cycle-free
   commits under a concurrent opposite-edge race.
4. **RUN-003** — condition evaluation (`completed_successfully`, `any_terminal`,
   `completed_or_cancelled`), the `ClaimReadyRun` handler with recovery and cancellation
   blocking, and a 100-way barrier race proving one winner.
5. **RUN-004** — the `CancelRun` handler: epoch increment, known-subtree propagation, terminal
   children untouched, and a barrier-controlled cancel/spawn race.
6. **RUN-005** — startup reconstruction: list non-terminal runs, classify effects/timers/bindings
   per the recovery matrix, persist dispositions, fail closed on unmapped combinations, and a
   thin `agentd` hook.

Dependency edges: RUN-000 → RUN-001 → RUN-002 → RUN-003 → RUN-004 → RUN-005 (RUN-004 also needs
RUN-001; RUN-003 and RUN-004 both need RUN-002).

### Rejected

| Alternative | Why not |
|---|---|
| A second parent-child edge table | Explicitly forbidden by `specs/run-graph.md`; ancestry is `runs.parent_run_id` only |
| Implementing `BindRun` here | Environment resolution belongs to the config module; this module only guards `Ready` |
| A dependency expression DSL | Forbidden in the MVP; three fixed conditions |
| In-database recursive CTE for descendants/children | The port has no raw SQL escape; an in-memory walk of `list_by_task` is correct at MVP scale, and a port method can replace it |
| Cancelling every non-terminal descendant directly to `Cancelled` | Loses the `Cancelling` state the transition table and events define |
| Auto-resuming recovery for any non-terminal run | Exit criterion: never blindly resume with an ambiguous external effect |

## Scope

**In scope**

- RUN-000 through RUN-005
- Port additions: `RunRead::list_active`, `EffectRead::list_by_run`, `TimerRead::list_by_run`
  (interface, SQLite implementation, testkit mock)
- Tests named in the briefs, plus registry/registration tests for the handlers

**Explicitly out of scope**

- Environment resolution and `BindRun` (config module)
- Loop turns and decision acceptance (loop module)
- Effect dispatch and reconciliation (effects module)
- Control API transport
- Schema changes; none are expected

## Capability map

Single capability — runtime entities, state, graph, claims, cancellation, recovery. Six
sequential tasks.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| Handler plumbing across command-core and persistence is integration-heavy | High | RUN-000 blocks everything | The first task proves one handler end to end with a real coordinator + SQLite store before the rest build on it |
| Port additions touch three completed crates (port, SQLite, mock) | Medium | Compile and mock parity | One task owns all three edits; the mock must stay faithful or later suites silently diverge |
| The 100-way claim race is flaky | Medium | Wasted review rounds | Barrier-controlled writers, busy_timeout tuning, assertions on database state not timing |
| Cancellation subtree semantics drift from the table | Medium | Recovery and lifecycle correctness | The state machine from RUN-001 is the only transition authority; cancellation composes it |
| Recovery classification precedence is under-specified | Medium | RUN-005 correctness | Assumption 7 fixes the precedence; unmapped combinations fail closed per the foundation ruling |

## Parallelisation forecast

Sequential after RUN-001; the module shares `create_run.rs`, repos, and graph services. The only
concurrency is inside tests.

## Open questions for the user

None blocking. Assumptions 3-8 are decisions taken under the standing instruction and are visible
here for correction.

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
