# Design — agentd-persistence

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

## Architecture

```
┌──────────────────────── consumers ────────────────────────┐
│ agentd::lock        identity::daemon      events::outbox  │
│ (OS file lock)      (fence lifecycle)     (stage events)  │
└───────────┬───────────────┬────────────────────┬──────────┘
            │               │                    │
            ▼               ▼                    ▼
┌──────────────────── kernel-store (port) ──────────────────┐
│ KernelStore · KernelTxn · KernelReadTxn · repo traits      │
│ models (row mirrors) · TxContext · DaemonFence             │
└───────────────────────────┬───────────────────────────────┘
                            ▼
┌────────────────── kernel-store-sqlite ────────────────────┐
│ SqliteKernelStore::open · schema bootstrap · BEGIN IMMEDIATE│
│ transaction guard · repos/* (parameterized SQL, CAS)       │
│ error mapping                                             │
└───────────────────────────┬───────────────────────────────┘
                            ▼
                       kernel.db (SQLite, WAL)
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| kernel-store | Traits, transaction context, row models, no SQL | existing stub, filled | `agent-os/crates/kernel-store/src/` |
| kernel-store-sqlite | Bootstrap, transactions, repositories, mapping | existing stub, filled | `agent-os/crates/kernel-store-sqlite/src/` |
| agentd lock | Exclusive OS lock guard | existing stub, filled | `agent-os/crates/agentd/src/lock.rs` |
| identity daemon | Fence claim and daemon-instance lifecycle | existing stub, filled | `agent-os/crates/identity/src/daemon.rs` |
| events outbox | Event staging API over the stream repo | existing stub, filled | `agent-os/crates/events/src/outbox.rs` |
| testkit store | In-memory mock of the transaction contract | new file | `agent-os/crates/testkit/src/store.rs` |

## Data flow

**Happy path — a write transaction**

1. `agentd` calls `DaemonLock::acquire(runtime_dir)`; the exclusive advisory lock on `agentd.lock` is the local single-writer gate.
2. `identity::daemon::acquire(&store, lease_ms)` calls `KernelStore::acquire_daemon_fence`, which commits a new `daemon_fence` row with `fencing_epoch + 1` for this instance.
3. A caller builds `TxContext { daemon_epoch, principal_id, command_id, correlation_id }` from the live fence.
4. `SqliteKernelStore::begin_write(ctx)` acquires a pooled connection and executes `BEGIN IMMEDIATE`; the returned `SqliteTxn` owns the connection and the epoch.
5. The txn reads the persisted `daemon_fence` and rejects with `FailedPrecondition` if the epoch is stale or missing.
6. Repositories run parameterized statements; CAS mutations use `UPDATE ... WHERE expected = ...` and report `Ok(false)` on zero rows.
7. `commit()` executes `COMMIT` and returns the connection to the pool; the outbox rows and stream-head increments caused by the mutation are visible atomically.

**Happy path — idempotent replay**

1. The command transaction calls `IdempotencyRepo::lookup(principal, key)`.
2. Absent: mutations run and `insert(record)` stores code and payload in the same transaction.
3. Present with the same digest: the stored outcome is returned and no mutation runs.
4. Present with a different digest: `Conflict`, no mutation.

**Failure path — constraint, busy, or invariant**

1. A constraint fires (`UNIQUE`, `CHECK`, foreign key, trigger `RAISE`).
2. The guard maps it: unique to `Conflict`, check/foreign-key/trigger to `FailedPrecondition`, busy or locked to `Unavailable` with retry-safe classification.
3. The guard issues `ROLLBACK`; the connection returns to the pool; zero partial rows remain.
4. If the transaction is dropped without an explicit outcome, `Drop` rolls back and swallows only the rollback error while the original failure propagates.
5. If decoding a persisted enum fails, the error is `Internal`, never retried, and the transaction refuses to commit.

## Interfaces

### kernel-store :: types.rs

```rust
#[derive(Clone, Debug)]
pub struct TxContext {
    pub daemon_epoch: u64,
    pub principal_id: PrincipalId,
    pub command_id: CommandId,
    pub correlation_id: Option⟨String⟩,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DaemonEpoch(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonFence {
    pub instance_id: DaemonInstanceId,
    pub epoch: DaemonEpoch,
    pub lease_expires_unix_ms: i64,
}
```

### kernel-store :: txn.rs

```rust
#[async_trait::async_trait]
pub trait KernelStore: Send + Sync {
    async fn begin_write(&self, ctx: TxContext) -> errors::Result⟨Box⟨dyn KernelTxn + '_⟩⟩;
    async fn begin_read(&self) -> errors::Result⟨Box⟨dyn KernelReadTxn + '_⟩⟩;
    async fn acquire_daemon_fence(&self, instance: DaemonInstanceId) -> errors::Result⟨DaemonFence⟩;
    async fn current_fence(&self) -> errors::Result⟨Option⟨DaemonFence⟩⟩;
}

#[async_trait::async_trait]
pub trait KernelTxn: Send + Sync {
    fn context(&self) -> &TxContext;
    fn runs(&mut self)         -> &mut dyn RunRepo;
    fn tasks(&mut self)        -> &mut dyn TaskRepo;
    fn sessions(&mut self)     -> &mut dyn SessionRepo;
    fn graph(&mut self)        -> &mut dyn GraphRepo;
    fn environments(&mut self) -> &mut dyn EnvironmentRepo;
    fn effects(&mut self)      -> &mut dyn EffectRepo;
    fn resources(&mut self)    -> &mut dyn ResourceRepo;
    fn timers(&mut self)       -> &mut dyn TimerRepo;
    fn security(&mut self)     -> &mut dyn SecurityRepo;
    fn config(&mut self)       -> &mut dyn ConfigRepo;
    fn workspaces(&mut self)   -> &mut dyn WorkspaceRepo;
    fn adapters(&mut self)     -> &mut dyn AdapterRepo;
    fn artifacts(&mut self)    -> &mut dyn ArtifactRepo;
    fn loop_turns(&mut self)   -> &mut dyn LoopRepo;
    fn idempotency(&mut self)  -> &mut dyn IdempotencyRepo;
    fn streams(&mut self)      -> &mut dyn StreamRepo;
    async fn commit(self: Box⟨Self⟩) -> errors::Result⟨()⟩;
    async fn rollback(self: Box⟨Self⟩) -> errors::Result⟨()⟩;
}

#[async_trait::async_trait]
pub trait KernelReadTxn: Send + Sync {
    fn runs(&mut self)         -> &mut dyn RunRead;
    fn tasks(&mut self)        -> &mut dyn TaskRead;
    fn sessions(&mut self)     -> &mut dyn SessionRead;
    fn graph(&mut self)        -> &mut dyn GraphRead;
    fn environments(&mut self) -> &mut dyn EnvironmentRead;
    fn effects(&mut self)      -> &mut dyn EffectRead;
    fn resources(&mut self)    -> &mut dyn ResourceRead;
    fn timers(&mut self)       -> &mut dyn TimerRead;
    fn security(&mut self)     -> &mut dyn SecurityRead;
    fn config(&mut self)       -> &mut dyn ConfigRead;
    fn workspaces(&mut self)   -> &mut dyn WorkspaceRead;
    fn adapters(&mut self)     -> &mut dyn AdapterRead;
    fn artifacts(&mut self)    -> &mut dyn ArtifactRead;
    fn loop_turns(&mut self)   -> &mut dyn LoopRead;
}
```

### kernel-store :: repositories.rs

Every write trait extends its read trait. Read trait methods take `&mut self`; write
methods add insert and compare-and-set operations.

```rust
#[async_trait::async_trait] pub trait RunRead: Send + Sync {
    async fn get(&mut self, id: RunId) -> Result⟨Option⟨RunRow⟩⟩;
    async fn list_by_task(&mut self, task: TaskId) -> Result⟨Vec⟨RunRow⟩⟩;
}
#[async_trait::async_trait] pub trait RunRepo: RunRead {
    async fn insert(&mut self, run: NewRun) -> Result⟨()⟩;
    async fn cas_update(&mut self, id: RunId, expect: RunCas, patch: RunPatch) -> Result⟨bool⟩;
}

pub struct RunCas { pub run_revision: u64, pub state: Option⟨RunState⟩, pub cancellation_epoch: Option⟨u64⟩ }
pub struct RunPatch {
    pub state: Option⟨RunState⟩,
    pub recovery: Option⟨RecoveryDisposition⟩,
    pub loop_epoch: Option⟨u64⟩,
    pub step_sequence: Option⟨u64⟩,
    pub input_event_cursor: Option⟨EventCursor⟩,
    pub cancellation_epoch: Option⟨u64⟩,
    pub resolved_environment_id: Option⟨EnvironmentId⟩,
    pub claim: Option⟨ClaimPatch⟩,
    pub terminal_reason: Option⟨String⟩,
    pub output_ref: Option⟨String⟩,
    pub current_turn_id: Option⟨TurnId⟩,
    pub bump_revision: bool,
}
pub struct ClaimPatch { pub owner: String, pub token: u64, pub expires_unix_ms: i64, pub daemon_epoch: u64 }
```

The remaining groups follow the same insert/get/list/cas pattern with these group-specific
operations:

| Trait | Group-specific operations |
|---|---|
| `GraphRepo` | `ensure_head(task)`, `get_head(task)`, `cas_head_revision(task, expected)`, `insert_dependency(new, expected_revision)`, `list_dependencies(task)`, `is_reachable(from, to)` |
| `EnvironmentRepo` | `insert_environment(new)`, `insert_bindings(environment, bindings)`, `get_environment(id)`, `get_bindings(id)`; no update methods |
| `EffectRepo` | `cas_transition(id, expect_state, expect_token, patch)` with `EffectPatch` covering state, executor, token, lease expiry, result and error refs |
| `ResourceRepo` | `cas_transition(id, expect_state, patch)` |
| `TimerRepo` | `cas_transition(id, expect_state, expect_version, patch)` |
| `SecurityRepo` | `insert_grant`, `get_grant`, `insert_delegation_hop`, `list_delegation_hops`, `insert_approval_request`, `get_approval_request`, `insert_approval_response`, `list_approval_responses` |
| `ConfigRepo` | `insert_generation`, `get_generation`, `get_active`, `cas_active(expected_revision, generation)` |
| `WorkspaceRepo` | `insert_workspace`, `insert_lease`, `get_lease`, `cas_lease(id, expect_epoch, patch)` |
| `AdapterRepo` | `insert_registration`, `get_registration`, `insert_instance`, `cas_instance_state`, `insert_conformance_report`, `get_conformance_report` |
| `ArtifactRepo` | `insert`, `get_by_id`, `get_by_uri` |
| `LoopRepo` | `insert_turn`, `get_turn`, `cas_turn(id, expect_state, patch)`, `insert_decision`, `get_decision(run, decision)` |
| `IdempotencyRepo` | `lookup(principal, key)`, `insert(record)` |
| `StreamRepo` | `allocate(stream_key)` returning the next sequence, `insert_outbox(new)`, `scan_unpublished(limit)` ordered by stream then sequence, `mark_published(event_id, kind)` |

All row types live in `models.rs` as field-for-field mirrors of the DDL columns with Rust
types from `domain` (ids, enums, cursors). `New*`, `*Cas`, and `*Patch` types accompany each
row. Example:

```rust
pub struct RunRow {
    pub run_id: RunId, pub task_id: TaskId, pub session_id: Option⟨SessionId⟩,
    pub parent_run_id: Option⟨RunId⟩, pub state: RunState,
    pub recovery: RecoveryDisposition, pub run_revision: u64, pub loop_epoch: u64,
    pub step_sequence: u64, pub input_event_cursor: EventCursor,
    pub cancellation_epoch: u64, pub resolved_environment_id: Option⟨EnvironmentId⟩,
    pub claim_owner: Option⟨String⟩, pub claim_token: Option⟨u64⟩,
    pub claim_expires_ms: Option⟨i64⟩, pub claim_daemon_epoch: Option⟨u64⟩,
    pub terminal_reason: Option⟨String⟩, pub output_ref: Option⟨String⟩,
    pub current_turn_id: Option⟨TurnId⟩, pub created_at_ms: i64, pub updated_at_ms: i64,
}
```

### kernel-store-sqlite

```rust
pub struct StoreConfig {
    pub path: std::path::PathBuf,
    pub pool_max_connections: u32,
    pub busy_timeout_ms: u64,
}

impl SqliteKernelStore {
    pub async fn open(config: StoreConfig) -> errors::Result⟨Self⟩;
    pub fn path(&self) -> &std::path::Path;
}
impl KernelStore for SqliteKernelStore { /* ... */ }
```

### agentd lock and identity daemon

```rust
// agent-os/crates/agentd/src/lock.rs
pub struct DaemonLock { /* File + path */ }
impl DaemonLock {
    pub fn acquire(runtime_dir: &std::path::Path) -> Result⟨Self, LockError⟩;
    pub fn release(self);
}
pub enum LockError { Held { path: std::path::PathBuf }, Io(std::io::Error) }

// agent-os/crates/identity/src/daemon.rs
pub struct DaemonHandle {
    pub instance_id: DaemonInstanceId,
    pub fence: kernel_store::DaemonFence,
}
pub async fn acquire(store: &dyn kernel_store::KernelStore, lease_ms: i64)
    -> errors::Result⟨DaemonHandle⟩;
```

### events outbox staging

```rust
// agent-os/crates/events/src/outbox.rs
pub struct DraftEvent {
    pub event_id: EventId,
    pub stream_key: EventStreamKey,
    pub event_type: String,
    pub payload: Vec⟨u8⟩,
    pub sensitivity: SensitivityClass,
    pub retention: RetentionClass,
    pub correlation_id: Option⟨String⟩,
    pub causation_id: Option⟨EventId⟩,
}
pub fn stage(txn: &mut dyn kernel_store::KernelTxn, draft: DraftEvent)
    -> errors::Result⟨u64⟩;   // returns the allocated sequence
```

## Data model

The data model is the inception schema, unchanged: `agent-os/schema/kernel_store.sql`
(installed verbatim by FND-002 from `specs/kernel-store-schema.sql`). This module adds no
tables, columns, constraints, or triggers.

| Aspect | Rule |
|---|---|
| Row structs | Field-for-field mirrors of DDL columns; nullability matches; ids and enums use `domain` types |
| Enum decode | Persisted integers decode through `domain` mirror enums; unknown values fail closed |
| TEXT state columns | Decode through the schema-checked string converters (`as_str`/`from_state_str`) |
| Immutability | `resolved_run_environments`, `resolved_bindings`, `agent_specs`, `approval_requests`, and `conformance_reports` reject updates at the storage layer (triggers) |
| Timestamps | UTC Unix milliseconds, `i64` |
| Sequences | `event_stream_heads.sequence` is the only source; allocation is in-transaction |
| Fencing | `daemon_fence.fencing_epoch` is monotonic; `runs.claim_daemon_epoch`, `effects.daemon_fencing_epoch`, and `timers.claim_daemon_epoch` record issuing epochs |

**Migration required:** no. **Backward compatible:** n/a — inception schema only.

## Error handling

| Failure mode | Detection | Mapping | Serves |
|---|---|---|---|
| Unique or primary-key violation | SQLite extended code for constraint unique | `Conflict`, `Never` | R3.4, R5.3 |
| Check, foreign key, not-null, or trigger abort | SQLite constraint codes and trigger message | `FailedPrecondition`, `Never` | R3.4 |
| Database busy or locked past timeout | SQLite busy/locked codes | `Unavailable`, `Safe` | R3.4, R4.3 |
| CAS matched zero rows | `rows_affected == 0` | `Ok(false)`; callers raise `Conflict` | R3.2 |
| Required row missing | Query returns none for a required lookup | `NotFound`, `Never` | R3.1 |
| Unknown persisted enum | Decode returns unknown-value error | `Internal`, `Never` | R3.1 |
| Schema version mismatch | Bootstrap read of `kernel_meta` | `FailedPrecondition`, `Never` | R1.3 |
| Busy timeout exhaustion in tests | Deterministic contention probe | `Unavailable`, `Safe` | N2 |
| Pool closed during shutdown | sqlx pool error | `Unavailable`, `Safe` | R4.4 |

**Error taxonomy:** `crates/errors` (`KernelError`, `ErrorCode`, `RetryClass`) from the
foundation module; no new codes are introduced. Repository code never returns sqlx errors
across the port.

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | n/a — local store, no network surface; principal identity is carried in `TxContext` for idempotency scoping only |
| Input validation and injection | all statements parameterized; no string-built SQL; no SQL escape hatch exposed by the port (R2.5) |
| Secrets handling | no secret material is stored or passed by this module; errors exclude payload bytes |
| Data exposure in logs / errors | trace spans carry ids and error codes only; `OutboxEvent` payloads are staged, never logged |
| New network surface | none |
| File permissions | `kernel.db` mode `0600`, runtime directory `0700`, lock file created `0600` (R1.5, N1) |
| Dependency additions | none — `sqlx`, `tokio`, and `async-trait` are already workspace dependencies; the lock uses `std::fs::File::try_lock` |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Unit | `cargo test` in-crate | `crates/kernel-store-sqlite/src/**` `#[cfg(test)]` | value encoding/decoding, error mapping |
| Integration | `cargo test` integration targets | `crates/kernel-store-sqlite/tests/` | bootstrap, version, CRUD, CAS, rollback, idempotency, outbox, fence |
| Concurrency | integration + testkit barriers | `crates/kernel-store-sqlite/tests/contention.rs`, `tests/outbox_concurrent.rs` | R3.5, R6.4, P2, P3 |
| Two-process lock | self-exec pattern | `crates/agentd/tests/lock_exclusion.rs` | R4.1, R4.6 |
| Property | `proptest` | `crates/kernel-store-sqlite/tests/props.rs` | P1, P2, P3, P4 |
| Mock contract | testkit | `crates/testkit/src/store.rs`, `crates/testkit/tests/store_mock.rs` | R2.6 |
| Regression | pack validators + workspace gates | repo root and `agent-os/` | G1, G2, N3 |

The two-process lock test uses the self-exec pattern: the test spawns its own test binary
with an environment marker and a lock-holder argument, so a real second process contends for
the OS lock without adding a production binary.

**Property tests**

| Property | Statement | Generator strategy |
|---|---|---|
| P1 | any write txn that fails or drops leaves zero rows from it | random operation sequences with injected failures at testkit fault points inside a temp database |
| P2 | for any race on one CAS expectation, exactly one commits | N writers on a barrier, random patches |
| P3 | concurrent appends to one stream produce contiguous unique sequences | M tasks staging random event bursts on one stream |
| P4 | replay with same principal/key/digest returns the stored outcome with unchanged row counts | random commands with a fixed digest and repeated submission counts |

**Explicitly not tested (and why):**

- Journal publication and live delivery — events module.
- Cross-database transactions — the architecture forbids them.
- Request digest computation — foundation spec.

## Observability

| Signal | Where | Content |
|---|---|---|
| `store.txn` span | begin_write/commit/rollback | daemon epoch, command id, principal id, outcome |
| `store.error` event | error mapping | stable code, retry class, table or constraint name; never payloads |
| `store.fence` span | fence acquire/assert | instance id, previous and new epoch |
| `store.busy` counter | busy mapping | bounded retries observed; used by the contention test diagnostics |

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N1 (security) | file modes asserted at bootstrap; static check that no port method exposes SQL | bootstrap test asserts modes; code review of the port surface |
| N2 (testability) | deterministic barriers and fault points; no sleeps | contention tests run under paused clocks; hygiene grep |
| N3 (compatibility) | no schema or contract edits | pack validators and lock check in CI |
| Writer throughput | `BEGIN IMMEDIATE` reserves ordering once per transaction; prepared statements cached by sqlx | contention test reports total commits under N writers |

No latency target is added by this module; correctness under contention is the measurable
property.

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | Runtime-checked `sqlx` queries with explicit row mapping | compile-time macros with offline metadata | keeps builds hermetic, matches the user's plan decision | R3 |
| D2 | The write guard issues `BEGIN IMMEDIATE` explicitly on its own pooled connection | relying on default deferred transactions; a global writer mutex | deferred transactions can lose CAS ordering under SQLite; the mutex would duplicate authority | R3.3 |
| D3 | Read and write repo traits are separate; write traits extend read traits | one trait with unused mutators in read txns | the type system enforces R2.2 instead of documentation | R2.2 |
| D4 | Row mirrors and patch/cas structs live in the `kernel-store` port crate | leaking sqlx row types or defining models in the SQLite crate | consumers compile against the port only; no sqlx types cross the boundary | R2.1, R2.5 |
| D5 | Busy/locked maps to `Unavailable` with `Safe`; constraints map to `Conflict`/`FailedPrecondition` | adding new error codes | the foundation taxonomy already distinguishes retry safety; no cross-module change | R3.4, N3 |
| D6 | Epoch assertion happens once at transaction start, not per statement | per-mutation assertion | the fence cannot change mid-transaction; asserting once is sufficient and cheaper | R4.3 |
| D7 | The daemon lock uses `std::fs::File::try_lock` with no new dependency | adding `fs2` | Rust 1.94 provides advisory locking; fewer supply-chain additions | R4.1 |
| D8 | Draining is in-memory: a lost fence refuses new transactions; nothing new is persisted | a `daemon_state` column | the persisted epoch is the authority; a second copy could disagree | R4.4 |
| D9 | Outbox rows are model types in the port; `events::outbox::stage` is the convenience API | outbox types in the events crate forcing a kernel-store dependency on it | dependency direction stays downward; staging remains ergonomic | R6.1 |
| D10 | Stream allocation uses one conditional update per event (`sequence = sequence + 1 ... RETURNING`) | read-then-write with application arithmetic | a gap or duplicate is impossible under the single statement; contiguity is structural | R6.2, R6.4 |
| D11 | Configuration comes from `StoreConfig` plus `AGENTD_HOME`; tests always use temp roots | hard-coded paths; global env injection | deterministic tests and explicit operator control | R1.1, N1 |
| D12 | Two-process lock proof uses the self-exec test pattern | adding a lock-holder binary | no production artifact exists only for tests | R4.1 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1.1–R1.5 | `schema.rs` bootstrap, `SqliteKernelStore::open`, pragmas and modes | `tests/bootstrap.rs` |
| R2.1–R2.5 | port traits, transaction guard, `Drop` rollback | `tests/txn_atomicity.rs`, object-safety compile test |
| R2.6 | testkit mock | `crates/testkit/tests/store_mock.rs` |
| R3.1 | models, enum and TEXT decoding | unit tests per repo |
| R3.2–R3.5 | CAS repositories, mapping, rollback | `tests/cas.rs`, `tests/contention.rs`, `tests/rollback.rs` |
| R3.6 | immutability triggers plus repository surface | `tests/immutability.rs` |
| R4.1–R4.6 | `DaemonLock`, `fence.rs`, `identity::daemon` | `tests/fence.rs`, `agentd/tests/lock_exclusion.rs` |
| R5.1–R5.5 | `IdempotencyRepo` | `tests/idempotency.rs` |
| R6.1–R6.5 | `StreamRepo`, `events::outbox` | `tests/outbox.rs`, `tests/outbox_concurrent.rs` |
| N1 | modes and port surface | bootstrap test, review |
| N2 | barriers and fault points | concurrency tests |
| N3 | no contract or schema edits | pack validators |
| P1 | zero partial rows after failure or drop | `tests/rollback.rs` |
| P2–P4 | property suite | `tests/props.rs` |
| G1 | quality gates | CI workflow |
| G2 | pack validators | repo root checks |

## File structure

Paths relative to `agent-os/`.

| Path | Create or modify | Responsibility | Owner task |
|---|---|---|---|
| `crates/kernel-store/src/lib.rs` | modify | module wiring, re-exports | PST-002 |
| `crates/kernel-store/src/types.rs` | create | `TxContext`, `DaemonEpoch`, `DaemonFence` | PST-002 |
| `crates/kernel-store/src/txn.rs` | create | `KernelStore`, `KernelTxn`, `KernelReadTxn` | PST-002 |
| `crates/kernel-store/src/repositories.rs` | create | read and write repo traits, cas and patch types | PST-002 |
| `crates/kernel-store/src/models.rs` | create | row mirrors, `New*` types | PST-002 |
| `crates/testkit/src/store.rs` | create | in-memory mock store and txn | PST-002 |
| `crates/testkit/src/lib.rs` | modify | declare `pub mod store;` | PST-002 |
| `crates/testkit/tests/store_mock.rs` | create | mock contract test | PST-002 |
| `crates/kernel-store-sqlite/src/lib.rs` | modify | `SqliteKernelStore::open`, pool, module wiring | PST-001 |
| `crates/kernel-store-sqlite/src/schema.rs` | create | bootstrap, version, pragmas, modes, structural assert | PST-001 |
| `crates/kernel-store-sqlite/tests/bootstrap.rs` | create | fresh, reopen, wrong version, modes, constraints | PST-001 |
| `crates/kernel-store-sqlite/src/txn.rs` | create | write guard, `BEGIN IMMEDIATE`, epoch assert, commit/rollback | PST-003A |
| `crates/kernel-store-sqlite/src/mapping.rs` | create | sqlx error to `KernelError` mapping, decode helpers | PST-003A |
| `crates/kernel-store-sqlite/src/repos/mod.rs` | create | repo module wiring | PST-003A |
| `crates/kernel-store-sqlite/src/repos/runs.rs` | create | run repository | PST-003A |
| `crates/kernel-store-sqlite/src/repos/tasks.rs` | create | task repository | PST-003A |
| `crates/kernel-store-sqlite/src/repos/sessions.rs` | create | session repository | PST-003A |
| `crates/kernel-store-sqlite/src/repos/graph.rs` | create | heads, dependencies, reachability | PST-003A |
| `crates/kernel-store-sqlite/src/repos/environments.rs` | create | immutable environment and bindings | PST-003A |
| `crates/kernel-store-sqlite/tests/cas.rs` | create | CAS conflict suite | PST-003A |
| `crates/kernel-store-sqlite/tests/rollback.rs` | create | atomicity and drop rollback | PST-003A |
| `crates/kernel-store-sqlite/tests/immutability.rs` | create | trigger-enforced immutability | PST-003A |
| `crates/kernel-store-sqlite/tests/contention.rs` | create | writer contention and single-winner CAS | PST-003A |
| `crates/kernel-store-sqlite/src/repos/effects.rs` | create | effect repository | PST-003B |
| `crates/kernel-store-sqlite/src/repos/resources.rs` | create | resource repository | PST-003B |
| `crates/kernel-store-sqlite/src/repos/timers.rs` | create | timer repository | PST-003B |
| `crates/kernel-store-sqlite/src/repos/security.rs` | create | grants, delegation, approvals | PST-003B |
| `crates/kernel-store-sqlite/src/repos/config.rs` | create | generations and active pointer | PST-003B |
| `crates/kernel-store-sqlite/src/repos/workspaces.rs` | create | workspaces and leases | PST-003B |
| `crates/kernel-store-sqlite/src/repos/adapters.rs` | create | registrations, instances, conformance | PST-003B |
| `crates/kernel-store-sqlite/src/repos/artifacts.rs` | create | artifact metadata | PST-003B |
| `crates/kernel-store-sqlite/src/repos/loop_turns.rs` | create | turns and decisions | PST-003B |
| `crates/kernel-store-sqlite/tests/repos_remaining.rs` | create | CRUD and CAS for the remaining groups | PST-003B |
| `crates/kernel-store-sqlite/src/fence.rs` | create | fence claim, read, assert, lease persistence | PST-004 |
| `crates/kernel-store-sqlite/tests/fence.rs` | create | epoch increment, stale rejection, lease | PST-004 |
| `crates/agentd/src/lock.rs` | modify | OS lock guard | PST-004 |
| `crates/agentd/tests/lock_exclusion.rs` | create | two-process lock exclusion (self-exec) | PST-004 |
| `crates/identity/src/daemon.rs` | modify | `DaemonHandle`, acquire | PST-004 |
| `crates/kernel-store-sqlite/src/repos/idempotency.rs` | create | lookup and insert | PST-005 |
| `crates/kernel-store-sqlite/src/repos/streams.rs` | create | allocate, outbox insert, scan, mark | PST-005 |
| `crates/kernel-store-sqlite/tests/idempotency.rs` | create | replay and conflict | PST-005 |
| `crates/kernel-store-sqlite/tests/outbox.rs` | create | ordering, uniqueness, atomic visibility | PST-005 |
| `crates/kernel-store-sqlite/tests/outbox_concurrent.rs` | create | contiguous concurrent sequences | PST-005 |
| `crates/events/src/outbox.rs` | modify | `DraftEvent`, `stage` | PST-005 |
| `crates/kernel-store-sqlite/tests/props.rs` | create | P2–P4 property suite | PST-005 |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
