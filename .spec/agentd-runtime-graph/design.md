# Design — agentd-runtime-graph

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

## Architecture

```
 control-api (later) ──envelope──▶ CommandCoordinator ──handler──▶ runtime handlers
                                                        │              │
                                        events::outbox ◀─┴──────────────┤
                                                                       ▼
                                              run-graph services (dependencies, readiness,
                                              cancellation) over kernel-store ports
                                                                       │
                                              kernel-store-sqlite repos (runs, tasks,
                                              sessions, agent_specs, graph, effects, timers)
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| Command handlers | Six catalogued commands as `CommandHandler`s | new fill | `agent-os/crates/runtime/src/{agent_spec,session,task,create_run,claim,cancel}.rs` |
| State machine | Transition table and revisioned mutations | new fill | `agent-os/crates/runtime/src/{state,run}.rs` |
| RunGraph services | Dependencies, readiness, cancellation, ancestry walks | new fill | `agent-os/crates/run-graph/src/{graph,repository,readiness,cancellation}.rs` |
| Recovery | Startup classification and dispositions | new fill | `agent-os/crates/runtime/src/recovery.rs`, `agent-os/crates/agentd/src/recovery.rs` |
| agent_specs repository | Insert-only immutable specs | new | `agent-os/crates/kernel-store-sqlite/src/repos/agent_specs.rs` |
| Port list additions | `RunRead::list_active`, `EffectRead::list_by_run`, `TimerRead::list_by_run` | modify | `agent-os/crates/kernel-store/src/repositories.rs`, SQLite repos, testkit mock |

## Data flow

**Happy path — create a run**

1. The control API submits `CreateTaskRun` through the coordinator; the handler decodes the prost payload.
2. Inside the already-fenced transaction, the handler validates the agent spec reference and, when a parent is present, loads the parent run and compares its `cancellation_epoch` with the observed value (mismatch or absence → `FailedPrecondition`).
3. The handler inserts the task when new together with its `run_graph_heads` row, then inserts the run in `Created` with `parent_run_id`.
4. It stages `TaskCreated`, `RunCreated`, and `ChildRunCreated` (child case) through `events::outbox::stage`.
5. The coordinator records the idempotency outcome and commits; the run remains `Created`.

**Happy path — dependency then claim**

1. `AddRunDependency` loads source and target, checks same task and mutable target, asks the graph repository to insert the edge (reachability + head revision + insert in the same immediate transaction), and stages `DependencyAdded`.
2. `BindRun` (config module) later persists the resolved environment and moves the run to `Ready`.
3. `ClaimReadyRun` evaluates every dependency condition, the recovery disposition, cancellation eligibility, and any live claim inside the transaction, then CAS-updates `Ready` to `Running` with owner, token, expiry, and daemon epoch, staging `RunClaimed` and `RunStarted`.

**Failure path — cancel/spawn race**

1. `CancelRun` increments the root's `cancellation_epoch` and transitions eligible descendants.
2. A concurrent `CreateTaskRun` holding the pre-advance observed epoch commits its check after the cancellation commit and is rejected with `FailedPrecondition`; no child row exists.

**Failure path — restart with ambiguity**

1. Startup enumerates non-terminal runs with effects, timers, and bindings.
2. A run whose effect is `UNKNOWN` or `DISPATCHED` without acknowledgment receives a blocked or reconciliation disposition; unmapped combinations fail startup closed.
3. No loop turn is issued for a non-`Normal` run.

## Interfaces

### runtime :: state.rs

```rust
/// True when the normative table permits this transition.
pub const fn allows(from: RunState, to: RunState) -> bool;
pub const fn is_terminal(state: RunState) -> bool;

pub const REASON_COMPLETED: &str = "completed";
pub const REASON_FAILED: &str = "failed";
pub const REASON_CANCELLED: &str = "cancelled";
pub const REASON_CANCELLATION_REQUESTED: &str = "cancellation_requested";
```

### runtime :: run.rs

```rust
/// Loads the run, enforces the transition table and expected revision,
/// applies the CAS update, and increments the revision exactly once.
pub async fn transition(
    txn: &mut dyn kernel_store::KernelTxn,
    run_id: domain::ids::RunId,
    expect_revision: u64,
    to: domain::RunState,
    reason: Option⟨String⟩,
    now_ms: i64,
) -> errors::Result⟨kernel_store::models::RunRow⟩;
```

### runtime :: handlers

```rust
pub struct RuntimeDeps {
    pub clock: std::sync::Arc⟨dyn domain::time::Clock⟩,
    pub ids: std::sync::Arc⟨dyn domain::provider::IdProvider⟩,
}

pub const CMD_CREATE_SESSION: &str = "agentos.spec.v1.CreateSession";
pub const CMD_PUT_AGENT_SPEC_REVISION: &str = "agentos.spec.v1.PutAgentSpecRevision";
pub const CMD_CREATE_TASK_RUN: &str = "agentos.spec.v1.CreateTaskRun";
pub const CMD_ADD_RUN_DEPENDENCY: &str = "agentos.spec.v1.AddRunDependency";
pub const CMD_CLAIM_READY_RUN: &str = "agentos.spec.v1.ClaimReadyRun";
pub const CMD_CANCEL_RUN: &str = "agentos.spec.v1.CancelRun";

pub struct CreateSessionHandler { /* deps */ }
pub struct PutAgentSpecRevisionHandler { /* deps */ }
pub struct CreateTaskRunHandler { /* deps */ }
pub struct AddRunDependencyHandler { /* deps */ }
pub struct ClaimReadyRunHandler { /* deps */ }
pub struct CancelRunHandler { /* deps */ }
// Each implements command_coordinator::handler::CommandHandler.

/// Registers all six handlers with their canonical command type strings.
pub fn register_handlers(
    registry: &mut command_coordinator::CommandRegistry,
    deps: RuntimeDeps,
) -> errors::Result⟨()⟩;
```

Service functions the handlers call (also unit-testable):

```rust
pub async fn create_session(txn, session_id: Option⟨SessionId⟩, principal: PrincipalId, now_ms) -> Result⟨SessionId⟩;
pub async fn put_agent_spec_revision(txn, spec_id: Option⟨AgentSpecId⟩, version: String, body: Vec⟨u8⟩, body_digest: String, now_ms) -> Result⟨()⟩;
pub async fn create_task_run(txn, request: CreateTaskRunRequest, ctx: &CommandContext, now_ms) -> Result⟨RunId⟩;
pub async fn claim(txn, run_id: RunId, owner: String, ttl_ms: u64, now_ms: i64, daemon_epoch: u64) -> Result⟨()⟩;
pub async fn cancel(txn, run_id: RunId, reason: &str, now_ms: i64) -> Result⟨CancellationReport⟩;
```

`CreateTaskRunRequest` mirrors the prost fields (task/run/session ids, kind, payload, spec ref,
parent, observed epoch, profile, workspace uri, capabilities, budget). `CancellationReport`
lists the runs that changed.

### run-graph

```rust
// graph.rs / repository.rs
pub async fn add_dependency(
    txn: &mut dyn KernelTxn, source: RunId, target: RunId,
    condition: DependencyCondition, expected_revision: u64, now_ms: i64,
) -> Result⟨()⟩;

/// Children by `runs.parent_run_id` within the task, walked in memory.
pub async fn descendants(txn: &mut dyn KernelTxn, root: RunId) -> Result⟨Vec⟨RunId⟩⟩;

// readiness.rs
pub const fn condition_met(condition: DependencyCondition, source_state: RunState) -> bool;
pub async fn dependencies_satisfied(txn, target: RunId) -> Result⟨bool⟩;

// cancellation.rs
pub async fn cancel_subtree(txn, root: RunId, reason: &str, now_ms: i64)
    -> Result⟨Vec⟨RunId⟩⟩;
```

### Recovery

```rust
pub struct RecoveryReport { pub examined: usize, pub dispositions: Vec⟨(RunId, RecoveryDisposition)⟩ }

pub async fn reconstruct(
    store: &dyn kernel_store::KernelStore,
    clock: std::sync::Arc⟨dyn domain::time::Clock⟩,
) -> Result⟨RecoveryReport⟩;

// agentd/src/recovery.rs
pub async fn run_startup_recovery(
    store: &dyn kernel_store::KernelStore,
    clock: std::sync::Arc⟨dyn domain::time::Clock⟩,
) -> Result⟨RecoveryReport⟩;   // thin call-through; wiring happens in INT-001
```

### Port additions (RUN-005)

```rust
pub trait RunRead: Send + Sync {
    /// Runs whose state is not terminal, ordered by creation.
    async fn list_active(&mut self) -> Result⟨Vec⟨RunRow⟩⟩;
}
pub trait EffectRead: Send + Sync {
    async fn list_by_run(&mut self, run_id: RunId) -> Result⟨Vec⟨EffectRow⟩⟩;
}
pub trait TimerRead: Send + Sync {
    async fn list_by_run(&mut self, run_id: RunId) -> Result⟨Vec⟨TimerRow⟩⟩;
}
```

## Data model

No schema changes; the tables exist with CHECK constraints and triggers. Recovery writes only
`runs.recovery_disposition` (and the revision), never run state. Graph mutations touch
`run_dependencies` and `run_graph_heads`. All mutations flow through the coordinator's handlers.

## Error handling

| Failure mode | Detection | Response | Serves |
|---|---|---|---|
| Illegal transition | table lookup | `Conflict`, `Never`, no mutation | R2.1 |
| Stale expected revision | CAS zero rows | `Conflict`, `Never` | R2.3 |
| AgentSpec revision rewrite | digest/body comparison | `Conflict`, `Never` | R1.2 |
| Missing or stale parent epoch | comparison with persisted | `FailedPrecondition`, `Never` | R1.5, R5.3 |
| Cross-task or immutable-target edge | validation | `FailedPrecondition`, `Never` | R3.1 |
| Cycle-forming edge | reachability | `Conflict`, `Never` | R3.2 |
| Unmet dependency at claim | condition evaluation | `FailedPrecondition`, `Never` | R4.2 |
| Live claim | unexpired claim check | `Conflict`, `Never` | R4.2 |
| Blocked recovery disposition | classification read | `FailedPrecondition`, `Never` | R4.6 |
| Unmapped recovery combination | matrix lookup | `Internal`, `Never`; startup fails closed | R6.3 |
| Unknown command payload | prost decode | `InvalidArgument`, `Never` | N1 |

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | principal comes from the coordinator context; capability checks arrive with later modules |
| Input validation and injection | payloads decode through generated prost types; no raw SQL outside SQLite |
| Secrets | none handled here |
| Data exposure | payload bytes never rendered into errors; tasks and runs carry opaque payload bytes |
| New network surface | none |
| Port additions | additive trait methods with SQLite and mock implementations behind unique/queries only |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Integration | real coordinator + SQLite store | `runtime/tests/{entities,claim,cancel,recovery}.rs` | R1, R4, R5, R6 |
| Unit/table | in-crate | `runtime/src/state.rs`, `run.rs` | R2 |
| Property | proptest | `runtime/tests/state_machine.rs` | R2, P1-adjacent |
| Graph | integration | `run-graph/tests/{graph,readiness,cancellation}.rs` | R3, R5 |
| Concurrency | barriers | claim 100-way, opposite-edge race, cancel/spawn race | P1, P2, P3 |
| Regression | gates and validators | repo root and `agent-os/` | G1, G2, N3 |

**Property tests**

| Property | Statement | Generator strategy |
|---|---|---|
| Transitions | any sequence of attempted transitions leaves the run in a legal state with a monotonically non-decreasing revision | arbitrary `(from, to)` sequences |
| P1 | any edge interleaving keeps the graph acyclic | sequential inserts with deliberate opposite pairs |
| P2 | any racing claims produce one winner | 100 barrier-synchronized tasks, one store |
| P3 | any cancel/spawn interleaving never commits a stale-epoch child | barrier-controlled pair |
| P4 | any ambiguous effect prevents a resumable disposition | table-driven effect states |

**Explicitly not tested (and why):**

- Environment resolution and `BindRun` — config module.
- Effect reconciliation execution — effects module.
- Worker scheduling — INT-001.

## Observability

| Signal | Where | Content |
|---|---|---|
| Handler errors | coordinator | stable codes; task/run ids, never payload bytes |
| `RecoveryReport` | agentd startup hook | counts and disposition list |
| Cancellation report | handler | changed run ids and epoch |

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N2 | barriers and property loops | test targets |
| Claim throughput | one CAS update per winner; losers exit early | 100-way test asserts one commit |
| Descendant walks | in-memory over `list_by_task` | MVP-scale graphs only; a port method replaces it later |

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | Handlers are stateless structs holding clock and id provider; principal comes from the command context | global state; principal in the payload | the coordinator already carries authenticated context | R1 |
| D2 | Command type constants are the fully-qualified prost message names | kebab strings | foundation D2 fixes the registry key | R1.1 |
| D3 | Ancestry walks are in-memory over `list_by_task` | recursive CTE; second edge table | no SQL escape hatch; spec forbids duplicating ancestry | R3.6 |
| D4 | Claim tests seed `Ready` through repository scaffolding, not a production path | adding a `ForceReady` command | `Ready` is produced only by `BindRun` | R4 |
| D5 | Recovery precedence: blocked unknown effect, then reconciliation, then missing resource, then recovering | first-match matrix order | matches the recovery table's safety ordering | R6.2 |
| D6 | Cancellation: `Created`/`Ready` move to `Cancelled`; started states to `Cancelling` | everything to `Cancelling` | never-started runs have no cleanup to drain | R5.2 |
| D7 | Three additive list methods only | broader query API | smallest surface recovery needs | R6.1 |
| D8 | Unmapped recovery combinations fail startup | default disposition | foundation ruling: fail closed | R6.3 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1.1–R1.7 | handlers and repos | `tests/entities.rs` |
| R2.1–R2.5 | state/run service | unit, table, property tests |
| R3.1–R3.6 | run-graph services | `run-graph/tests/graph.rs` |
| R4.1–R4.6 | readiness and claim | `tests/claim.rs` |
| R5.1–R5.5 | cancellation | `tests/cancel.rs` |
| R6.1–R6.6 | recovery | `tests/recovery.rs` |
| N1–N3 | shapes and gates | suites, validators |
| P1–P4 | property and race suites | test targets |
| G1, G2 | gates and validators | repo root and `agent-os/` |

## File structure

Paths relative to `agent-os/`.

| Path | Create or modify | Responsibility | Owner task |
|---|---|---|---|
| `crates/runtime/Cargo.toml`, `crates/run-graph/Cargo.toml`, `crates/agentd/Cargo.toml`, `Cargo.lock` | modify | dependencies (incl. proptest dev) | RUN-000 |
| `crates/kernel-store-sqlite/src/repos/agent_specs.rs` | create | insert-only spec repository | RUN-000 |
| `crates/kernel-store-sqlite/src/repos/mod.rs` | modify | declare `agent_specs` | RUN-000 |
| `crates/runtime/src/lib.rs` | modify | modules and `register_handlers` | RUN-000, RUN-003, RUN-004 |
| `crates/runtime/src/{agent_spec,session,task,create_run}.rs` | create | entity handlers and services | RUN-000 |
| `crates/runtime/tests/entities.rs` | create | entity suite | RUN-000 |
| `crates/runtime/src/{state,run}.rs` | create | state machine and transition service | RUN-001 |
| `crates/runtime/tests/state_machine.rs` | create | table and property tests | RUN-001 |
| `crates/run-graph/src/lib.rs` | modify | module declarations | RUN-002, RUN-003, RUN-004 |
| `crates/run-graph/src/{graph,repository}.rs` | create | dependency service and queries | RUN-002 |
| `crates/run-graph/tests/graph.rs` | create | edge suite | RUN-002 |
| `crates/run-graph/src/readiness.rs` | create | conditions and dependency evaluation | RUN-003 |
| `crates/runtime/src/claim.rs` | create | claim service and handler | RUN-003 |
| `crates/run-graph/tests/readiness.rs`, `crates/runtime/tests/claim.rs` | create | readiness and claim suites | RUN-003 |
| `crates/run-graph/src/cancellation.rs` | create | subtree cancellation | RUN-004 |
| `crates/runtime/src/cancel.rs` | create | cancel service and handler | RUN-004 |
| `crates/run-graph/tests/cancellation.rs`, `crates/runtime/tests/cancel.rs` | create | cancellation suites | RUN-004 |
| `crates/kernel-store/src/repositories.rs` | modify | three list methods | RUN-005 |
| `crates/kernel-store-sqlite/src/repos/{runs,effects,timers}.rs` | modify | list implementations | RUN-005 |
| `crates/testkit/src/store.rs` | modify | mock list implementations | RUN-005 |
| `crates/runtime/src/recovery.rs`, `crates/agentd/src/recovery.rs` | create/modify | reconstruction and hook | RUN-005 |
| `crates/runtime/tests/recovery.rs` | create | recovery suite | RUN-005 |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
