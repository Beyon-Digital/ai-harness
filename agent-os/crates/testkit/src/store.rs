//! In-memory implementation of the `kernel_store` port for tests.
//!
//! `MockStore` mirrors the observable contract of the SQLite store without
//! SQL: write transactions mutate a private snapshot, commit publishes the
//! snapshot atomically, drop or rollback discards it, CAS operations check the
//! expected revision/state/token, idempotency keys are unique, and stream
//! allocation hands out contiguous sequences per stream key. Write
//! transactions assert the persisted daemon epoch at begin, like the storage
//! layer (R4.3, D6).
//!
//! Two behaviours are intentionally coarser than SQLite and are documented on
//! [`MockStore`]: the epoch assertion rejects absent or stale epochs instead
//! of comparing a live fence lease, and commit is guarded by a single
//! whole-store revision, so any overlapping writer conflicts rather than
//! receiving the row-level serialization `BEGIN IMMEDIATE` would provide.
//!
//! Beyond those two, the mock emulates the DDL constraints that later modules
//! exercise: the partial unique index on active exclusive-write leases, the
//! TEXT CHECK literal sets for the untyped persisted enums, and the foreign
//! keys listed on [`MockStore`]. Every constraint that is still not emulated
//! is called out in that list so mock-based tests do not rely on it.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use domain::ids::{
    AdapterId, AdapterInstanceId, AgentSpecId, ApprovalRequestId, ArtifactId, CapabilityGrantId,
    ConfigGenerationId, DaemonInstanceId, DecisionId, DelegationChainId, DependencyId, EffectId,
    EnvironmentId, EventId, EventStreamKey, IdempotencyKey, LeaseId, PrincipalId, ReservationId,
    RunId, SessionId, TaskId, TimerId, TurnId, WorkspaceId,
};
use domain::resource::{LeaseEnforcementState, WorkspaceAccessMode};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::*;
use kernel_store::repositories::*;
use kernel_store::types::{DaemonEpoch, DaemonFence, TxContext};
use kernel_store::{KernelReadTxn, KernelStore, KernelTxn};

#[derive(Clone, Default)]
struct MockState {
    revision: u64,
    logical_now_ms: i64,
    daemon_epoch: u64,
    daemon_fence: Option<DaemonFence>,
    sessions: HashMap<SessionId, SessionRow>,
    agent_specs: HashMap<(AgentSpecId, String), AgentSpecRow>,
    tasks: HashMap<TaskId, TaskRow>,
    environments: HashMap<EnvironmentId, ResolvedEnvironmentRow>,
    bindings: HashMap<(EnvironmentId, String), ResolvedBindingRow>,
    runs: HashMap<RunId, RunRow>,
    graph_heads: HashMap<TaskId, RunGraphHeadRow>,
    dependencies: HashMap<DependencyId, RunDependencyRow>,
    workspaces: HashMap<WorkspaceId, WorkspaceRow>,
    leases: HashMap<LeaseId, WorkspaceLeaseRow>,
    effects: HashMap<EffectId, EffectRow>,
    reservations: HashMap<ReservationId, ReservationRow>,
    timers: HashMap<TimerId, TimerRow>,
    grants: HashMap<CapabilityGrantId, CapabilityGrantRow>,
    delegation_hops: HashMap<(DelegationChainId, u32), DelegationHopRow>,
    approval_requests: HashMap<ApprovalRequestId, ApprovalRequestRow>,
    approval_responses: HashMap<ApprovalRequestId, ApprovalResponseRow>,
    registrations: HashMap<(AdapterId, String, String), AdapterRegistrationRow>,
    instances: HashMap<AdapterInstanceId, AdapterInstanceRow>,
    conformance_reports: HashMap<(AdapterId, String, String), ConformanceReportRow>,
    artifacts_by_id: HashMap<ArtifactId, ArtifactRow>,
    artifacts_by_uri: HashMap<String, ArtifactId>,
    config_generations: HashMap<ConfigGenerationId, ConfigGenerationRow>,
    active_config: Option<ActiveConfigGenerationRow>,
    turns: HashMap<TurnId, LoopTurnRow>,
    decisions: HashMap<(RunId, DecisionId), DecisionRow>,
    idempotency: HashMap<(PrincipalId, IdempotencyKey), IdempotencyRecordRow>,
    stream_heads: HashMap<EventStreamKey, u64>,
    outbox: HashMap<EventId, OutboxEventRow>,
    outbox_sequences: HashSet<(EventStreamKey, u64)>,
}

/// In-memory `KernelStore` double.
///
/// The persisted daemon epoch starts at `0`, meaning no daemon fence has been
/// recorded. Use [`MockStore::set_daemon_epoch`] or
/// [`KernelStore::acquire_daemon_fence`] before opening a write transaction:
/// `begin_write` rejects an absent (`0`) or mismatched context epoch with
/// `FailedPrecondition`, mirroring the storage-layer epoch assertion (R4.3,
/// D6).
///
/// Constraint emulation:
///
/// - a second active exclusive-write lease for one workspace is rejected with
///   `Conflict`, mirroring `uq_workspace_exclusive_lease`;
/// - the persisted-literal CHECK sets are validated for loop turns
///   (`issued|accepted|stale`), decisions (`Complete|Fail|Wait|SpawnAgent|
///   InvokeEffect|RequestApproval`), adapter instances (`starting|ready|
///   exited|failed`), config generations (`proposed|validated|rejected` and
///   `untested|passed|failed`), conformance reports (`pass|fail`), and
///   approval responses (`approve|deny`), on insert and on state patches,
///   failing `FailedPrecondition`/`Never`;
/// - agent spec inserts are insert-only: an identical `(id, version, digest,
///   body)` is idempotent, a differing body or digest conflicts, and one
///   digest cannot bind to two revisions, mirroring the schema key and unique
///   constraints;
/// - foreign-key existence is checked for artifacts (origin run and effect),
///   timers (run), leases (workspace and owner run), turns (run), decisions
///   (run and turn), grants (run and delegating grant), delegation hops
///   (run), and dependency endpoints (source and target run).
///
/// Still unchecked, so mock-based tests must not rely on these being
/// rejected: the immutability triggers (the port exposes no update or delete
/// for those tables), CHECK constraints on typed enum and integer columns
/// (unrepresentable through the port types), `workspaces.parent_workspace_id`,
/// `approval_requests.run_id`, outbox entity references, delegation-hop chain
/// contiguity, and any foreign key not listed above.
///
/// Concurrency limitation: commits are guarded by one whole-store revision.
/// Any overlapping write transaction started from an older revision fails
/// with `Conflict`/`Safe` even when CAS expectations match and the rows
/// touched are disjoint. The SQLite store serializes writers with
/// `BEGIN IMMEDIATE` and would permit disjoint or non-conflicting commits, so
/// mock-based concurrency tests must stay conservative and must not assume
/// that two overlapping writers both succeed.
#[derive(Clone, Default)]
pub struct MockStore {
    state: Arc<Mutex<MockState>>,
}

impl MockStore {
    /// Creates an empty store with persisted daemon epoch `0` (no fence).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the persisted daemon epoch and fence, simulating state left by
    /// [`KernelStore::acquire_daemon_fence`]. Pass `0` to clear the fence.
    /// Any change bumps the store revision, so in-flight write transactions
    /// fail their commit with `Conflict` and cannot restore the old epoch.
    /// The initial value is `0` with no fence; a write transaction presenting
    /// epoch `0` or a non-matching epoch is rejected with
    /// `FailedPrecondition`.
    pub fn set_daemon_epoch(&self, epoch: u64, instance: DaemonInstanceId) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if epoch == 0 {
            state.daemon_epoch = 0;
            state.daemon_fence = None;
            state.revision = state.revision.saturating_add(1);
        } else {
            install_fence(&mut state, epoch, instance);
        }
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> errors::Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| internal("mock store lock poisoned"))
}

fn internal(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Internal, RetryClass::Never, message)
}

fn conflict(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Conflict, RetryClass::Never, message)
}

fn not_found(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::NotFound, RetryClass::Never, message)
}

fn failed_precondition(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, message)
}

/// Fails closed when an untyped literal violates its schema CHECK set.
fn check_literal(column: &'static str, value: &str, allowed: &[&str]) -> errors::Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("{column} has an unknown literal: {value}"),
        ))
    }
}

/// Emulates `uq_workspace_exclusive_lease`: at most one active exclusive-write
/// lease may exist per workspace.
fn check_exclusive_lease(
    state: &MockState,
    workspace: WorkspaceId,
    mode: WorkspaceAccessMode,
    enforcement_state: LeaseEnforcementState,
    exclude: Option<LeaseId>,
) -> errors::Result<()> {
    let would_be_active = mode == WorkspaceAccessMode::ExclusiveWrite
        && enforcement_state == LeaseEnforcementState::Active;
    let occupied = would_be_active
        && state.leases.values().any(|row| {
            row.workspace_id == workspace
                && row.mode == WorkspaceAccessMode::ExclusiveWrite
                && row.enforcement_state == LeaseEnforcementState::Active
                && Some(row.lease_id) != exclude
        });
    if occupied {
        return Err(conflict(
            "exclusive write lease already active for the workspace",
        ));
    }
    Ok(())
}

/// Directed reachability over the snapshot's dependency edges.
fn reaches(state: &MockState, from: RunId, to: RunId) -> bool {
    if from == to {
        return true;
    }
    let mut adjacency: HashMap<RunId, Vec<RunId>> = HashMap::new();
    for dependency in state.dependencies.values() {
        adjacency
            .entry(dependency.source_run_id)
            .or_default()
            .push(dependency.target_run_id);
    }
    let mut seen: HashSet<RunId> = HashSet::new();
    let mut queue: VecDeque<RunId> = VecDeque::new();
    queue.push_back(from);
    seen.insert(from);
    while let Some(current) = queue.pop_front() {
        if let Some(targets) = adjacency.get(&current) {
            for target in targets {
                if *target == to {
                    return true;
                }
                if seen.insert(*target) {
                    queue.push_back(*target);
                }
            }
        }
    }
    false
}

/// Persists a fence and bumps the store revision, invalidating any write
/// transaction opened before the change.
fn install_fence(state: &mut MockState, epoch: u64, instance: DaemonInstanceId) -> DaemonFence {
    let fence = DaemonFence {
        instance_id: instance,
        epoch: DaemonEpoch(epoch),
        lease_expires_unix_ms: i64::MAX,
    };
    state.daemon_epoch = epoch;
    state.daemon_fence = Some(fence.clone());
    state.revision = state.revision.saturating_add(1);
    fence
}

fn insert_unique<K, V>(
    map: &mut HashMap<K, V>,
    key: K,
    value: V,
    message: &'static str,
) -> errors::Result<()>
where
    K: std::hash::Hash + Eq,
{
    match map.entry(key) {
        Entry::Occupied(_) => Err(conflict(message)),
        Entry::Vacant(slot) => {
            slot.insert(value);
            Ok(())
        }
    }
}

macro_rules! repo_handle {
    ($($name:ident),+ $(,)?) => {
        $(
            struct $name {
                state: Arc<Mutex<MockState>>,
            }

            impl $name {
                fn new(state: Arc<Mutex<MockState>>) -> Self {
                    Self { state }
                }
            }
        )+
    };
}

repo_handle!(
    MockRunRepo,
    MockTaskRepo,
    MockSessionRepo,
    MockAgentSpecRepo,
    MockGraphRepo,
    MockEnvironmentRepo,
    MockEffectRepo,
    MockResourceRepo,
    MockTimerRepo,
    MockSecurityRepo,
    MockConfigRepo,
    MockWorkspaceRepo,
    MockAdapterRepo,
    MockArtifactRepo,
    MockLoopRepo,
    MockIdempotencyRepo,
    MockStreamRepo,
);

struct MockWriteTxn {
    ctx: TxContext,
    base_revision: u64,
    state: Arc<Mutex<MockState>>,
    store: Arc<Mutex<MockState>>,
    runs: MockRunRepo,
    tasks: MockTaskRepo,
    sessions: MockSessionRepo,
    agent_specs: MockAgentSpecRepo,
    graph: MockGraphRepo,
    environments: MockEnvironmentRepo,
    effects: MockEffectRepo,
    resources: MockResourceRepo,
    timers: MockTimerRepo,
    security: MockSecurityRepo,
    config: MockConfigRepo,
    workspaces: MockWorkspaceRepo,
    adapters: MockAdapterRepo,
    artifacts: MockArtifactRepo,
    loop_turns: MockLoopRepo,
    idempotency: MockIdempotencyRepo,
    streams: MockStreamRepo,
}

impl MockWriteTxn {
    fn new(
        ctx: TxContext,
        base_revision: u64,
        store: Arc<Mutex<MockState>>,
        state: Arc<Mutex<MockState>>,
    ) -> Self {
        Self {
            ctx,
            base_revision,
            state: state.clone(),
            store,
            runs: MockRunRepo::new(state.clone()),
            tasks: MockTaskRepo::new(state.clone()),
            sessions: MockSessionRepo::new(state.clone()),
            agent_specs: MockAgentSpecRepo::new(state.clone()),
            graph: MockGraphRepo::new(state.clone()),
            environments: MockEnvironmentRepo::new(state.clone()),
            effects: MockEffectRepo::new(state.clone()),
            resources: MockResourceRepo::new(state.clone()),
            timers: MockTimerRepo::new(state.clone()),
            security: MockSecurityRepo::new(state.clone()),
            config: MockConfigRepo::new(state.clone()),
            workspaces: MockWorkspaceRepo::new(state.clone()),
            adapters: MockAdapterRepo::new(state.clone()),
            artifacts: MockArtifactRepo::new(state.clone()),
            loop_turns: MockLoopRepo::new(state.clone()),
            idempotency: MockIdempotencyRepo::new(state.clone()),
            streams: MockStreamRepo::new(state),
        }
    }
}

struct MockReadTxn {
    runs: MockRunRepo,
    tasks: MockTaskRepo,
    sessions: MockSessionRepo,
    agent_specs: MockAgentSpecRepo,
    graph: MockGraphRepo,
    environments: MockEnvironmentRepo,
    effects: MockEffectRepo,
    resources: MockResourceRepo,
    timers: MockTimerRepo,
    security: MockSecurityRepo,
    config: MockConfigRepo,
    workspaces: MockWorkspaceRepo,
    adapters: MockAdapterRepo,
    artifacts: MockArtifactRepo,
    loop_turns: MockLoopRepo,
}

impl MockReadTxn {
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self {
            runs: MockRunRepo::new(state.clone()),
            tasks: MockTaskRepo::new(state.clone()),
            sessions: MockSessionRepo::new(state.clone()),
            agent_specs: MockAgentSpecRepo::new(state.clone()),
            graph: MockGraphRepo::new(state.clone()),
            environments: MockEnvironmentRepo::new(state.clone()),
            effects: MockEffectRepo::new(state.clone()),
            resources: MockResourceRepo::new(state.clone()),
            timers: MockTimerRepo::new(state.clone()),
            security: MockSecurityRepo::new(state.clone()),
            config: MockConfigRepo::new(state.clone()),
            workspaces: MockWorkspaceRepo::new(state.clone()),
            adapters: MockAdapterRepo::new(state.clone()),
            artifacts: MockArtifactRepo::new(state.clone()),
            loop_turns: MockLoopRepo::new(state),
        }
    }
}

#[async_trait]
impl KernelStore for MockStore {
    async fn begin_write(&self, ctx: TxContext) -> errors::Result<Box<dyn KernelTxn + '_>> {
        let snapshot = {
            let state = lock(&self.state)?;
            if ctx.daemon_epoch == 0 || ctx.daemon_epoch != state.daemon_epoch {
                return Err(failed_precondition(
                    "write transaction rejected: daemon epoch is absent or stale",
                ));
            }
            state.clone()
        };
        let base_revision = snapshot.revision;
        let txn_state = Arc::new(Mutex::new(snapshot));
        Ok(Box::new(MockWriteTxn::new(
            ctx,
            base_revision,
            self.state.clone(),
            txn_state,
        )))
    }

    async fn begin_read(&self) -> errors::Result<Box<dyn KernelReadTxn + '_>> {
        let snapshot = {
            let state = lock(&self.state)?;
            state.clone()
        };
        Ok(Box::new(MockReadTxn::new(Arc::new(Mutex::new(snapshot)))))
    }

    async fn acquire_daemon_fence(
        &self,
        instance: DaemonInstanceId,
    ) -> errors::Result<DaemonFence> {
        let mut state = lock(&self.state)?;
        let epoch = state.daemon_epoch.saturating_add(1);
        Ok(install_fence(&mut state, epoch, instance))
    }

    async fn current_fence(&self) -> errors::Result<Option<DaemonFence>> {
        Ok(lock(&self.state)?.daemon_fence.clone())
    }
}

#[async_trait]
impl KernelTxn for MockWriteTxn {
    fn context(&self) -> &TxContext {
        &self.ctx
    }

    fn runs(&mut self) -> &mut dyn RunRepo {
        &mut self.runs
    }

    fn tasks(&mut self) -> &mut dyn TaskRepo {
        &mut self.tasks
    }

    fn sessions(&mut self) -> &mut dyn SessionRepo {
        &mut self.sessions
    }

    fn agent_specs(&mut self) -> &mut dyn AgentSpecRepo {
        &mut self.agent_specs
    }

    fn graph(&mut self) -> &mut dyn GraphRepo {
        &mut self.graph
    }

    fn environments(&mut self) -> &mut dyn EnvironmentRepo {
        &mut self.environments
    }

    fn effects(&mut self) -> &mut dyn EffectRepo {
        &mut self.effects
    }

    fn resources(&mut self) -> &mut dyn ResourceRepo {
        &mut self.resources
    }

    fn timers(&mut self) -> &mut dyn TimerRepo {
        &mut self.timers
    }

    fn security(&mut self) -> &mut dyn SecurityRepo {
        &mut self.security
    }

    fn config(&mut self) -> &mut dyn ConfigRepo {
        &mut self.config
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRepo {
        &mut self.workspaces
    }

    fn adapters(&mut self) -> &mut dyn AdapterRepo {
        &mut self.adapters
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRepo {
        &mut self.artifacts
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRepo {
        &mut self.loop_turns
    }

    fn idempotency(&mut self) -> &mut dyn IdempotencyRepo {
        &mut self.idempotency
    }

    fn streams(&mut self) -> &mut dyn StreamRepo {
        &mut self.streams
    }

    async fn commit(self: Box<Self>) -> errors::Result<()> {
        let txn = *self;
        let mut store = lock(&txn.store)?;
        if store.revision != txn.base_revision {
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Safe,
                "mock store revision changed during the transaction",
            ));
        }
        let snapshot = {
            let state = lock(&txn.state)?;
            state.clone()
        };
        let mut next = snapshot;
        next.revision = store.revision.saturating_add(1);
        *store = next;
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> errors::Result<()> {
        drop(self);
        Ok(())
    }
}

#[async_trait]
impl KernelReadTxn for MockReadTxn {
    fn runs(&mut self) -> &mut dyn RunRead {
        &mut self.runs
    }

    fn tasks(&mut self) -> &mut dyn TaskRead {
        &mut self.tasks
    }

    fn sessions(&mut self) -> &mut dyn SessionRead {
        &mut self.sessions
    }

    fn agent_specs(&mut self) -> &mut dyn AgentSpecRead {
        &mut self.agent_specs
    }

    fn graph(&mut self) -> &mut dyn GraphRead {
        &mut self.graph
    }

    fn environments(&mut self) -> &mut dyn EnvironmentRead {
        &mut self.environments
    }

    fn effects(&mut self) -> &mut dyn EffectRead {
        &mut self.effects
    }

    fn resources(&mut self) -> &mut dyn ResourceRead {
        &mut self.resources
    }

    fn timers(&mut self) -> &mut dyn TimerRead {
        &mut self.timers
    }

    fn security(&mut self) -> &mut dyn SecurityRead {
        &mut self.security
    }

    fn config(&mut self) -> &mut dyn ConfigRead {
        &mut self.config
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRead {
        &mut self.workspaces
    }

    fn adapters(&mut self) -> &mut dyn AdapterRead {
        &mut self.adapters
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRead {
        &mut self.artifacts
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRead {
        &mut self.loop_turns
    }
}

#[async_trait]
impl RunRead for MockRunRepo {
    async fn get(&mut self, id: RunId) -> errors::Result<Option<RunRow>> {
        Ok(lock(&self.state)?.runs.get(&id).cloned())
    }

    async fn list_by_task(&mut self, task: TaskId) -> errors::Result<Vec<RunRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<RunRow> = state
            .runs
            .values()
            .filter(|row| row.task_id == task)
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.run_id);
        Ok(rows)
    }

    async fn list_active(&mut self) -> errors::Result<Vec<RunRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<RunRow> = state
            .runs
            .values()
            .filter(|row| !row.state.is_terminal())
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.created_at_ms, row.run_id));
        Ok(rows)
    }
}

#[async_trait]
impl RunRepo for MockRunRepo {
    async fn insert(&mut self, run: NewRun) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if !state.tasks.contains_key(&run.task_id) {
            return Err(failed_precondition("run references a missing task"));
        }
        if let Some(parent) = run.parent_run_id
            && !state.runs.contains_key(&parent)
        {
            return Err(failed_precondition("run references a missing parent run"));
        }
        let row = RunRow {
            run_id: run.run_id,
            task_id: run.task_id,
            session_id: run.session_id,
            parent_run_id: run.parent_run_id,
            state: run.state,
            recovery: run.recovery,
            run_revision: 0,
            loop_epoch: run.loop_epoch,
            step_sequence: run.step_sequence,
            input_event_cursor: run.input_event_cursor,
            cancellation_epoch: run.cancellation_epoch,
            resolved_environment_id: run.resolved_environment_id,
            claim_owner: None,
            claim_token: None,
            claim_expires_ms: None,
            claim_daemon_epoch: None,
            terminal_reason: None,
            output_ref: None,
            current_turn_id: None,
            created_at_ms: run.created_at_ms,
            updated_at_ms: run.created_at_ms,
        };
        insert_unique(&mut state.runs, row.run_id, row, "duplicate run id")
    }

    async fn cas_update(
        &mut self,
        id: RunId,
        expect: RunCas,
        patch: RunPatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(run) = state.runs.get_mut(&id) else {
            return Ok(false);
        };
        if run.run_revision != expect.run_revision {
            return Ok(false);
        }
        if let Some(expected) = expect.state
            && run.state != expected
        {
            return Ok(false);
        }
        if let Some(expected) = expect.cancellation_epoch
            && run.cancellation_epoch != expected
        {
            return Ok(false);
        }
        if let Some(value) = patch.state {
            run.state = value;
        }
        if let Some(value) = patch.recovery {
            run.recovery = value;
        }
        if let Some(value) = patch.loop_epoch {
            run.loop_epoch = value;
        }
        if let Some(value) = patch.step_sequence {
            run.step_sequence = value;
        }
        if let Some(value) = patch.input_event_cursor {
            run.input_event_cursor = value;
        }
        if let Some(value) = patch.cancellation_epoch {
            run.cancellation_epoch = value;
        }
        if let Some(value) = patch.resolved_environment_id {
            run.resolved_environment_id = Some(value);
        }
        if let Some(claim) = patch.claim {
            run.claim_owner = Some(claim.owner);
            run.claim_token = Some(claim.token);
            run.claim_expires_ms = Some(claim.expires_unix_ms);
            run.claim_daemon_epoch = Some(claim.daemon_epoch);
        }
        if let Some(value) = patch.terminal_reason {
            run.terminal_reason = Some(value);
        }
        if let Some(value) = patch.output_ref {
            run.output_ref = Some(value);
        }
        if let Some(value) = patch.current_turn_id {
            run.current_turn_id = Some(value);
        }
        if patch.bump_revision {
            run.run_revision = run.run_revision.saturating_add(1);
        }
        Ok(true)
    }
}

#[async_trait]
impl TaskRead for MockTaskRepo {
    async fn get(&mut self, id: TaskId) -> errors::Result<Option<TaskRow>> {
        Ok(lock(&self.state)?.tasks.get(&id).cloned())
    }

    async fn list_by_session(&mut self, session: SessionId) -> errors::Result<Vec<TaskRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<TaskRow> = state
            .tasks
            .values()
            .filter(|row| row.session_id == Some(session))
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.task_id);
        Ok(rows)
    }
}

#[async_trait]
impl TaskRepo for MockTaskRepo {
    async fn insert(&mut self, task: NewTask) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if let Some(session) = task.session_id
            && !state.sessions.contains_key(&session)
        {
            return Err(failed_precondition("task references a missing session"));
        }
        let row = TaskRow {
            task_id: task.task_id,
            session_id: task.session_id,
            created_by_actor_id: task.created_by_actor_id,
            task_kind: task.task_kind,
            payload: task.payload,
            created_at_ms: task.created_at_ms,
        };
        insert_unique(&mut state.tasks, row.task_id, row, "duplicate task id")
    }
}

#[async_trait]
impl SessionRead for MockSessionRepo {
    async fn get(&mut self, id: SessionId) -> errors::Result<Option<SessionRow>> {
        Ok(lock(&self.state)?.sessions.get(&id).cloned())
    }
}

#[async_trait]
impl SessionRepo for MockSessionRepo {
    async fn insert(&mut self, session: NewSession) -> errors::Result<()> {
        let row = SessionRow {
            session_id: session.session_id,
            principal_id: session.principal_id,
            created_at_ms: session.created_at_ms,
            metadata: session.metadata,
        };
        insert_unique(
            &mut lock(&self.state)?.sessions,
            row.session_id,
            row,
            "duplicate session id",
        )
    }
}

#[async_trait]
impl AgentSpecRead for MockAgentSpecRepo {
    async fn get(
        &mut self,
        id: AgentSpecId,
        version: &str,
    ) -> errors::Result<Option<AgentSpecRow>> {
        Ok(lock(&self.state)?
            .agent_specs
            .get(&(id, version.to_owned()))
            .cloned())
    }
}

#[async_trait]
impl AgentSpecRepo for MockAgentSpecRepo {
    async fn insert(&mut self, spec: NewAgentSpec) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if let Some(stored) = state
            .agent_specs
            .get(&(spec.agent_spec_id, spec.version.clone()))
        {
            if stored.digest == spec.digest && stored.body == spec.body {
                return Ok(());
            }
            return Err(conflict(
                "agent spec revision conflicts with the stored revision",
            ));
        }
        if state
            .agent_specs
            .values()
            .any(|row| row.digest == spec.digest)
        {
            return Err(conflict(
                "agent spec digest is already bound to another revision",
            ));
        }
        let row = AgentSpecRow {
            agent_spec_id: spec.agent_spec_id,
            version: spec.version,
            digest: spec.digest,
            body: spec.body,
            created_at_ms: spec.created_at_ms,
        };
        state
            .agent_specs
            .insert((row.agent_spec_id, row.version.clone()), row);
        Ok(())
    }
}

#[async_trait]
impl GraphRead for MockGraphRepo {
    async fn get_head(&mut self, task: TaskId) -> errors::Result<Option<RunGraphHeadRow>> {
        Ok(lock(&self.state)?.graph_heads.get(&task).cloned())
    }

    async fn list_dependencies(&mut self, task: TaskId) -> errors::Result<Vec<RunDependencyRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<RunDependencyRow> = state
            .dependencies
            .values()
            .filter(|row| row.task_id == task)
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.created_graph_revision, row.dependency_id));
        Ok(rows)
    }

    async fn is_reachable(&mut self, from: RunId, to: RunId) -> errors::Result<bool> {
        let state = lock(&self.state)?;
        Ok(reaches(&state, from, to))
    }
}

#[async_trait]
impl GraphRepo for MockGraphRepo {
    async fn ensure_head(&mut self, task: TaskId) -> errors::Result<RunGraphHeadRow> {
        let mut state = lock(&self.state)?;
        let row = state
            .graph_heads
            .entry(task)
            .or_insert_with(|| RunGraphHeadRow {
                task_id: task,
                graph_revision: 0,
            });
        Ok(row.clone())
    }

    async fn cas_head_revision(&mut self, task: TaskId, expected: u64) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(head) = state.graph_heads.get_mut(&task) else {
            return Ok(false);
        };
        if head.graph_revision != expected {
            return Ok(false);
        }
        head.graph_revision = head.graph_revision.saturating_add(1);
        Ok(true)
    }

    async fn insert_dependency(
        &mut self,
        new: NewRunDependency,
        expected_revision: u64,
    ) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        let Some(head) = state.graph_heads.get(&new.task_id) else {
            return Err(not_found("graph head missing for task"));
        };
        if head.graph_revision != expected_revision {
            return Err(conflict("graph revision conflict"));
        }
        if !state.runs.contains_key(&new.source_run_id)
            || !state.runs.contains_key(&new.target_run_id)
        {
            return Err(failed_precondition("dependency references a missing run"));
        }
        if reaches(&state, new.target_run_id, new.source_run_id) {
            return Err(conflict("dependency would create a cycle"));
        }
        for dependency in state.dependencies.values() {
            if dependency.source_run_id == new.source_run_id
                && dependency.target_run_id == new.target_run_id
            {
                return Err(conflict("duplicate run dependency"));
            }
        }
        let row = RunDependencyRow {
            dependency_id: new.dependency_id,
            task_id: new.task_id,
            source_run_id: new.source_run_id,
            target_run_id: new.target_run_id,
            dependency_condition: new.dependency_condition,
            created_graph_revision: expected_revision,
            created_at_ms: new.created_at_ms,
        };
        insert_unique(
            &mut state.dependencies,
            row.dependency_id,
            row,
            "duplicate dependency id",
        )?;
        if let Some(head) = state.graph_heads.get_mut(&new.task_id) {
            head.graph_revision = head.graph_revision.saturating_add(1);
        }
        Ok(())
    }
}

#[async_trait]
impl EnvironmentRead for MockEnvironmentRepo {
    async fn get_environment(
        &mut self,
        id: EnvironmentId,
    ) -> errors::Result<Option<ResolvedEnvironmentRow>> {
        Ok(lock(&self.state)?.environments.get(&id).cloned())
    }

    async fn get_bindings(&mut self, id: EnvironmentId) -> errors::Result<Vec<ResolvedBindingRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<ResolvedBindingRow> = state
            .bindings
            .values()
            .filter(|row| row.environment_id == id)
            .cloned()
            .collect();
        rows.sort_by(|left, right| left.port_id.cmp(&right.port_id));
        Ok(rows)
    }
}

#[async_trait]
impl EnvironmentRepo for MockEnvironmentRepo {
    async fn insert_environment(&mut self, new: NewResolvedEnvironment) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if state
            .environments
            .values()
            .any(|row| row.run_id == new.run_id)
        {
            return Err(conflict("run already has a resolved environment"));
        }
        let row = ResolvedEnvironmentRow {
            environment_id: new.environment_id,
            run_id: new.run_id,
            agent_spec_id: new.agent_spec_id,
            agent_spec_version: new.agent_spec_version,
            agent_spec_digest: new.agent_spec_digest,
            agent_loop_id: new.agent_loop_id,
            agent_loop_version: new.agent_loop_version,
            agent_loop_digest: new.agent_loop_digest,
            config_generation_id: new.config_generation_id,
            workspace_uri: new.workspace_uri,
            workspace_base_revision: new.workspace_base_revision,
            workspace_mode: new.workspace_mode,
            model_provider: new.model_provider,
            model_id: new.model_id,
            model_parameters: new.model_parameters,
            kernel_version: new.kernel_version,
            protocol_versions: new.protocol_versions,
            capability_grant_ids: new.capability_grant_ids,
            approval_request_ids: new.approval_request_ids,
            created_at_ms: new.created_at_ms,
        };
        insert_unique(
            &mut state.environments,
            row.environment_id,
            row,
            "duplicate environment id",
        )
    }

    async fn insert_bindings(
        &mut self,
        environment: EnvironmentId,
        bindings: Vec<NewResolvedBinding>,
    ) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if !state.environments.contains_key(&environment) {
            return Err(failed_precondition(
                "bindings reference a missing environment",
            ));
        }
        for binding in bindings {
            let row = ResolvedBindingRow {
                environment_id: environment,
                port_id: binding.port_id,
                adapter_id: binding.adapter_id,
                adapter_version: binding.adapter_version,
                adapter_digest: binding.adapter_digest,
                capabilities: binding.capabilities,
            };
            insert_unique(
                &mut state.bindings,
                (environment, row.port_id.clone()),
                row,
                "duplicate resolved binding",
            )?;
        }
        Ok(())
    }
}

#[async_trait]
impl EffectRead for MockEffectRepo {
    async fn get(&mut self, id: EffectId) -> errors::Result<Option<EffectRow>> {
        Ok(lock(&self.state)?.effects.get(&id).cloned())
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<EffectRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<EffectRow> = state
            .effects
            .values()
            .filter(|row| row.run_id == run)
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.step_sequence, row.effect_id));
        Ok(rows)
    }
}

#[async_trait]
impl EffectRepo for MockEffectRepo {
    async fn insert(&mut self, effect: NewEffect) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if !state.runs.contains_key(&effect.run_id) {
            return Err(failed_precondition("effect references a missing run"));
        }
        if state.effects.values().any(|row| {
            row.run_id == effect.run_id
                && row.decision_id == effect.decision_id
                && row.operation == effect.operation
                && row.request_hash == effect.request_hash
        }) {
            return Err(conflict("duplicate effect identity"));
        }
        let row = EffectRow {
            effect_id: effect.effect_id,
            run_id: effect.run_id,
            step_sequence: effect.step_sequence,
            decision_id: effect.decision_id,
            operation: effect.operation,
            request_hash: effect.request_hash,
            request_payload: effect.request_payload,
            effect_class: effect.effect_class,
            idempotency_semantics: effect.idempotency_semantics,
            reconciliation_semantics: effect.reconciliation_semantics,
            cancellation_semantics: effect.cancellation_semantics,
            compensation_capability: effect.compensation_capability,
            adapter_id: effect.adapter_id,
            adapter_version: effect.adapter_version,
            adapter_digest: effect.adapter_digest,
            state: effect.state,
            executor_id: None,
            executor_fencing_token: None,
            daemon_fencing_epoch: None,
            lease_expires_ms: None,
            provider_operation_ref: None,
            result_ref: None,
            error_code: None,
            created_at_ms: effect.created_at_ms,
            updated_at_ms: effect.created_at_ms,
        };
        insert_unique(
            &mut state.effects,
            row.effect_id,
            row,
            "duplicate effect id",
        )
    }

    async fn claim(
        &mut self,
        id: EffectId,
        executor_id: &str,
        daemon_epoch: u64,
        lease_expires_ms: i64,
        now_ms: i64,
    ) -> errors::Result<Option<u64>> {
        let mut state = lock(&self.state)?;
        let Some(effect) = state.effects.get_mut(&id) else {
            return Ok(None);
        };
        let eligible = effect.state == domain::effect::EffectState::Prepared
            || (effect.state == domain::effect::EffectState::Claimed
                && effect.lease_expires_ms.is_some_and(|lease| lease <= now_ms));
        if !eligible {
            return Ok(None);
        }
        let token = effect.executor_fencing_token.unwrap_or(0) + 1;
        effect.state = domain::effect::EffectState::Claimed;
        effect.executor_id = Some(executor_id.to_owned());
        effect.executor_fencing_token = Some(token);
        effect.daemon_fencing_epoch = Some(daemon_epoch);
        effect.lease_expires_ms = Some(lease_expires_ms);
        effect.updated_at_ms = now_ms;
        Ok(Some(token))
    }

    async fn cas_transition(
        &mut self,
        id: EffectId,
        expect_state: domain::effect::EffectState,
        expect_token: Option<u64>,
        patch: EffectPatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(effect) = state.effects.get_mut(&id) else {
            return Ok(false);
        };
        if effect.state != expect_state {
            return Ok(false);
        }
        if let Some(token) = expect_token
            && effect.executor_fencing_token != Some(token)
        {
            return Ok(false);
        }
        if let Some(value) = patch.state {
            effect.state = value;
        }
        if let Some(value) = patch.executor_id {
            effect.executor_id = Some(value);
        }
        if let Some(value) = patch.executor_fencing_token {
            effect.executor_fencing_token = Some(value);
        }
        if let Some(value) = patch.daemon_fencing_epoch {
            effect.daemon_fencing_epoch = Some(value);
        }
        if let Some(value) = patch.lease_expires_ms {
            effect.lease_expires_ms = Some(value);
        }
        if let Some(value) = patch.provider_operation_ref {
            effect.provider_operation_ref = Some(value);
        }
        if let Some(value) = patch.result_ref {
            effect.result_ref = Some(value);
        }
        if let Some(value) = patch.error_code {
            effect.error_code = Some(value);
        }
        Ok(true)
    }
}

#[async_trait]
impl ResourceRead for MockResourceRepo {
    async fn get(&mut self, id: ReservationId) -> errors::Result<Option<ReservationRow>> {
        Ok(lock(&self.state)?.reservations.get(&id).cloned())
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<ReservationRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<ReservationRow> = state
            .reservations
            .values()
            .filter(|row| row.run_id == run)
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.reservation_id);
        Ok(rows)
    }

    async fn list_children(
        &mut self,
        parent: ReservationId,
    ) -> errors::Result<Vec<ReservationRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<ReservationRow> = state
            .reservations
            .values()
            .filter(|row| row.parent_reservation_id == Some(parent))
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.reservation_id);
        Ok(rows)
    }
}

#[async_trait]
impl ResourceRepo for MockResourceRepo {
    async fn insert(&mut self, reservation: NewReservation) -> errors::Result<()> {
        let row = ReservationRow {
            reservation_id: reservation.reservation_id,
            run_id: reservation.run_id,
            resource_type: reservation.resource_type,
            state: reservation.state,
            amount: reservation.amount,
            unit: reservation.unit,
            fencing_token: reservation.fencing_token,
            parent_reservation_id: reservation.parent_reservation_id,
            created_at_ms: reservation.created_at_ms,
            updated_at_ms: reservation.created_at_ms,
        };
        insert_unique(
            &mut lock(&self.state)?.reservations,
            row.reservation_id,
            row,
            "duplicate reservation id",
        )
    }

    async fn cas_transition(
        &mut self,
        id: ReservationId,
        expect_state: domain::resource::ReservationState,
        patch: ReservationPatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(reservation) = state.reservations.get_mut(&id) else {
            return Ok(false);
        };
        if reservation.state != expect_state {
            return Ok(false);
        }
        if let Some(value) = patch.state {
            reservation.state = value;
        }
        if let Some(value) = patch.fencing_token {
            reservation.fencing_token = value;
        }
        Ok(true)
    }
}

#[async_trait]
impl TimerRead for MockTimerRepo {
    async fn get(&mut self, id: TimerId) -> errors::Result<Option<TimerRow>> {
        Ok(lock(&self.state)?.timers.get(&id).cloned())
    }

    async fn list_due(&mut self, due_before_ms: i64) -> errors::Result<Vec<TimerRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<TimerRow> = state
            .timers
            .values()
            .filter(|row| row.due_at_ms <= due_before_ms)
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.due_at_ms, row.timer_id));
        Ok(rows)
    }

    async fn list_by_run(&mut self, run_id: RunId) -> errors::Result<Vec<TimerRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<TimerRow> = state
            .timers
            .values()
            .filter(|row| row.run_id == Some(run_id))
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.due_at_ms, row.timer_id));
        Ok(rows)
    }
}

#[async_trait]
impl TimerRepo for MockTimerRepo {
    async fn insert(&mut self, timer: NewTimer) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if let Some(run) = timer.run_id
            && !state.runs.contains_key(&run)
        {
            return Err(failed_precondition("timer references a missing run"));
        }
        let row = TimerRow {
            timer_id: timer.timer_id,
            run_id: timer.run_id,
            timer_kind: timer.timer_kind,
            payload: timer.payload,
            due_at_ms: timer.due_at_ms,
            state: timer.state,
            version: timer.version,
            claim_owner: None,
            claim_fencing_token: None,
            claim_daemon_epoch: None,
            created_at_ms: timer.created_at_ms,
            updated_at_ms: timer.created_at_ms,
        };
        insert_unique(&mut state.timers, row.timer_id, row, "duplicate timer id")
    }

    async fn cas_transition(
        &mut self,
        id: TimerId,
        expect_state: domain::resource::TimerState,
        expect_version: u64,
        patch: TimerPatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(timer) = state.timers.get_mut(&id) else {
            return Ok(false);
        };
        if timer.state != expect_state || timer.version != expect_version {
            return Ok(false);
        }
        if let Some(value) = patch.state {
            timer.state = value;
        }
        if let Some(value) = patch.claim_owner {
            timer.claim_owner = Some(value);
        }
        if let Some(value) = patch.claim_fencing_token {
            timer.claim_fencing_token = Some(value);
        }
        if let Some(value) = patch.claim_daemon_epoch {
            timer.claim_daemon_epoch = Some(value);
        }
        if let Some(value) = patch.due_at_ms {
            timer.due_at_ms = value;
        }
        timer.version = timer.version.saturating_add(1);
        Ok(true)
    }
}

#[async_trait]
impl SecurityRead for MockSecurityRepo {
    async fn get_grant(
        &mut self,
        id: CapabilityGrantId,
    ) -> errors::Result<Option<CapabilityGrantRow>> {
        Ok(lock(&self.state)?.grants.get(&id).cloned())
    }

    async fn list_delegation_hops(
        &mut self,
        chain: DelegationChainId,
    ) -> errors::Result<Vec<DelegationHopRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<DelegationHopRow> = state
            .delegation_hops
            .values()
            .filter(|row| row.chain_id == chain)
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.hop_index);
        Ok(rows)
    }

    async fn get_approval_request(
        &mut self,
        id: ApprovalRequestId,
    ) -> errors::Result<Option<ApprovalRequestRow>> {
        Ok(lock(&self.state)?.approval_requests.get(&id).cloned())
    }

    async fn list_approvals_by_run(
        &mut self,
        run_id: RunId,
    ) -> errors::Result<Vec<ApprovalRequestRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<ApprovalRequestRow> = state
            .approval_requests
            .values()
            .filter(|row| row.run_id == Some(run_id))
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.created_at_ms, row.request_id));
        Ok(rows)
    }

    async fn list_approval_responses(
        &mut self,
        request: ApprovalRequestId,
    ) -> errors::Result<Vec<ApprovalResponseRow>> {
        let state = lock(&self.state)?;
        let rows: Vec<ApprovalResponseRow> = state
            .approval_responses
            .values()
            .filter(|row| row.request_id == request)
            .cloned()
            .collect();
        Ok(rows)
    }
}

#[async_trait]
impl SecurityRepo for MockSecurityRepo {
    async fn insert_grant(&mut self, grant: NewCapabilityGrant) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if let Some(run) = grant.run_id
            && !state.runs.contains_key(&run)
        {
            return Err(failed_precondition("grant references a missing run"));
        }
        if let Some(parent) = grant.delegated_from_grant_id
            && !state.grants.contains_key(&parent)
        {
            return Err(failed_precondition(
                "grant references a missing parent grant",
            ));
        }
        let row = CapabilityGrantRow {
            grant_id: grant.grant_id,
            principal_id: grant.principal_id,
            actor_id: grant.actor_id,
            run_id: grant.run_id,
            capability_id: grant.capability_id,
            scope: grant.scope,
            delegated_from_grant_id: grant.delegated_from_grant_id,
            expires_at_ms: grant.expires_at_ms,
            revoked_at_ms: grant.revoked_at_ms,
            created_at_ms: grant.created_at_ms,
        };
        insert_unique(&mut state.grants, row.grant_id, row, "duplicate grant id")
    }

    async fn insert_delegation_hop(&mut self, hop: NewDelegationHop) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if let Some(run) = hop.run_id
            && !state.runs.contains_key(&run)
        {
            return Err(failed_precondition(
                "delegation hop references a missing run",
            ));
        }
        let row = DelegationHopRow {
            chain_id: hop.chain_id,
            hop_index: hop.hop_index,
            principal_or_actor_id: hop.principal_or_actor_id,
            run_id: hop.run_id,
            capability_grant_ids: hop.capability_grant_ids,
        };
        insert_unique(
            &mut state.delegation_hops,
            (row.chain_id, row.hop_index),
            row,
            "duplicate delegation hop",
        )
    }

    async fn insert_approval_request(&mut self, request: NewApprovalRequest) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if state
            .approval_requests
            .values()
            .any(|row| row.request_digest == request.request_digest && row.nonce == request.nonce)
        {
            return Err(conflict("duplicate approval request digest and nonce"));
        }
        let row = ApprovalRequestRow {
            request_id: request.request_id,
            request_digest: request.request_digest,
            principal_id: request.principal_id,
            actor_id: request.actor_id,
            run_id: request.run_id,
            operation: request.operation,
            target_resource: request.target_resource,
            capability_ids: request.capability_ids,
            extension_bundle_digest: request.extension_bundle_digest,
            config_generation_digest: request.config_generation_digest,
            expires_at_ms: request.expires_at_ms,
            nonce: request.nonce,
            state: request.state,
            created_at_ms: request.created_at_ms,
            resolved_at_ms: request.resolved_at_ms,
        };
        insert_unique(
            &mut state.approval_requests,
            row.request_id,
            row,
            "duplicate approval request id",
        )
    }

    async fn insert_approval_response(
        &mut self,
        response: NewApprovalResponse,
    ) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "approval_responses.decision",
            &response.decision,
            &["approve", "deny"],
        )?;
        if !state.approval_requests.contains_key(&response.request_id) {
            return Err(failed_precondition(
                "approval response references a missing request",
            ));
        }
        let row = ApprovalResponseRow {
            request_id: response.request_id,
            request_digest: response.request_digest,
            decision: response.decision,
            device_id: response.device_id,
            responder_principal_id: response.responder_principal_id,
            responded_at_ms: response.responded_at_ms,
        };
        insert_unique(
            &mut state.approval_responses,
            row.request_id,
            row,
            "duplicate approval response",
        )
    }
}

#[async_trait]
impl ConfigRead for MockConfigRepo {
    async fn get_generation(
        &mut self,
        id: ConfigGenerationId,
    ) -> errors::Result<Option<ConfigGenerationRow>> {
        Ok(lock(&self.state)?.config_generations.get(&id).cloned())
    }

    async fn get_active(&mut self) -> errors::Result<Option<ActiveConfigGenerationRow>> {
        Ok(lock(&self.state)?.active_config.clone())
    }
}

#[async_trait]
impl ConfigRepo for MockConfigRepo {
    async fn insert_generation(&mut self, generation: NewConfigGeneration) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "config_generations.validation_state",
            &generation.validation_state,
            &["proposed", "validated", "rejected"],
        )?;
        check_literal(
            "config_generations.test_state",
            &generation.test_state,
            &["untested", "passed", "failed"],
        )?;
        if state
            .config_generations
            .values()
            .any(|row| row.digest == generation.digest)
        {
            return Err(conflict("duplicate config generation digest"));
        }
        let row = ConfigGenerationRow {
            generation_id: generation.generation_id,
            digest: generation.digest,
            document: generation.document,
            validation_state: generation.validation_state,
            test_state: generation.test_state,
            created_by_actor_id: generation.created_by_actor_id,
            created_at_ms: generation.created_at_ms,
        };
        insert_unique(
            &mut state.config_generations,
            row.generation_id,
            row,
            "duplicate config generation id",
        )
    }

    async fn set_generation_states(
        &mut self,
        id: ConfigGenerationId,
        validation_state: Option<&str>,
        test_state: Option<&str>,
    ) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        let Some(row) = state.config_generations.get_mut(&id) else {
            return Err(not_found("config generation not found"));
        };
        if let Some(value) = validation_state {
            check_literal(
                "config_generations.validation_state",
                value,
                &["proposed", "validated", "rejected"],
            )?;
            row.validation_state = value.to_owned();
        }
        if let Some(value) = test_state {
            check_literal(
                "config_generations.test_state",
                value,
                &["untested", "passed", "failed"],
            )?;
            row.test_state = value.to_owned();
        }
        Ok(())
    }

    async fn cas_active(
        &mut self,
        expected_revision: u64,
        generation: ConfigGenerationId,
        activated_at_ms: i64,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let current = state
            .active_config
            .as_ref()
            .map(|active| active.revision)
            .unwrap_or(0);
        if current != expected_revision {
            return Ok(false);
        }
        if !state.config_generations.contains_key(&generation) {
            return Err(not_found("config generation not found"));
        }
        state.active_config = Some(ActiveConfigGenerationRow {
            generation_id: generation,
            revision: expected_revision.saturating_add(1),
            activated_at_ms,
        });
        Ok(true)
    }
}

#[async_trait]
impl WorkspaceRead for MockWorkspaceRepo {
    async fn get_workspace(&mut self, id: WorkspaceId) -> errors::Result<Option<WorkspaceRow>> {
        Ok(lock(&self.state)?.workspaces.get(&id).cloned())
    }

    async fn get_lease(&mut self, id: LeaseId) -> errors::Result<Option<WorkspaceLeaseRow>> {
        Ok(lock(&self.state)?.leases.get(&id).cloned())
    }
}

#[async_trait]
impl WorkspaceRepo for MockWorkspaceRepo {
    async fn insert_workspace(&mut self, workspace: NewWorkspace) -> errors::Result<()> {
        let row = WorkspaceRow {
            workspace_id: workspace.workspace_id,
            kind: workspace.kind,
            base_revision: workspace.base_revision,
            parent_workspace_id: workspace.parent_workspace_id,
            created_at_ms: workspace.created_at_ms,
        };
        insert_unique(
            &mut lock(&self.state)?.workspaces,
            row.workspace_id,
            row,
            "duplicate workspace id",
        )
    }

    async fn insert_lease(&mut self, lease: NewWorkspaceLease) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if !state.workspaces.contains_key(&lease.workspace_id) {
            return Err(failed_precondition("lease references a missing workspace"));
        }
        if !state.runs.contains_key(&lease.owner_run_id) {
            return Err(failed_precondition("lease references a missing owner run"));
        }
        check_exclusive_lease(
            &state,
            lease.workspace_id,
            lease.mode,
            lease.enforcement_state,
            None,
        )?;
        let row = WorkspaceLeaseRow {
            lease_id: lease.lease_id,
            workspace_id: lease.workspace_id,
            owner_run_id: lease.owner_run_id,
            mode: lease.mode,
            lease_epoch: lease.lease_epoch,
            enforcement_state: lease.enforcement_state,
            delegated_from: lease.delegated_from,
            created_at_ms: lease.created_at_ms,
            updated_at_ms: lease.created_at_ms,
        };
        insert_unique(&mut state.leases, row.lease_id, row, "duplicate lease id")
    }

    async fn cas_lease(
        &mut self,
        id: LeaseId,
        expect_epoch: u64,
        patch: LeasePatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(lease) = state.leases.get(&id).cloned() else {
            return Ok(false);
        };
        if lease.lease_epoch != expect_epoch {
            return Ok(false);
        }
        let mode = patch.mode.unwrap_or(lease.mode);
        let enforcement_state = patch.enforcement_state.unwrap_or(lease.enforcement_state);
        if let Some(owner) = patch.owner_run_id
            && !state.runs.contains_key(&owner)
        {
            return Err(failed_precondition("lease references a missing owner run"));
        }
        check_exclusive_lease(
            &state,
            lease.workspace_id,
            mode,
            enforcement_state,
            Some(id),
        )?;
        let lease = state.leases.get_mut(&id).expect("lease checked above");
        if let Some(value) = patch.enforcement_state {
            lease.enforcement_state = value;
        }
        if let Some(value) = patch.owner_run_id {
            lease.owner_run_id = value;
        }
        if let Some(value) = patch.mode {
            lease.mode = value;
        }
        if let Some(value) = patch.delegated_from {
            lease.delegated_from = value;
        }
        if let Some(epoch) = patch.lease_epoch {
            lease.lease_epoch = epoch;
        }
        Ok(true)
    }
}

#[async_trait]
impl AdapterRead for MockAdapterRepo {
    async fn get_registration(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> errors::Result<Option<AdapterRegistrationRow>> {
        Ok(lock(&self.state)?
            .registrations
            .get(&(adapter_id, version.to_owned(), bundle_digest.to_owned()))
            .cloned())
    }

    async fn list_registrations(&mut self) -> errors::Result<Vec<AdapterRegistrationRow>> {
        let mut rows: Vec<_> = lock(&self.state)?.registrations.values().cloned().collect();
        rows.sort_by(|a, b| {
            (&a.adapter_id, &a.version, &a.bundle_digest).cmp(&(
                &b.adapter_id,
                &b.version,
                &b.bundle_digest,
            ))
        });
        Ok(rows)
    }

    async fn get_conformance_report(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> errors::Result<Option<ConformanceReportRow>> {
        Ok(lock(&self.state)?
            .conformance_reports
            .get(&(adapter_id, version.to_owned(), bundle_digest.to_owned()))
            .cloned())
    }

    async fn get_instance(
        &mut self,
        adapter_instance_id: AdapterInstanceId,
    ) -> errors::Result<Option<AdapterInstanceRow>> {
        Ok(lock(&self.state)?
            .instances
            .get(&adapter_instance_id)
            .cloned())
    }
}

#[async_trait]
impl AdapterRepo for MockAdapterRepo {
    async fn insert_registration(
        &mut self,
        registration: NewAdapterRegistration,
    ) -> errors::Result<()> {
        let row = AdapterRegistrationRow {
            adapter_id: registration.adapter_id,
            version: registration.version,
            bundle_digest: registration.bundle_digest,
            manifest_digest: registration.manifest_digest,
            runtime_type: registration.runtime_type,
            implemented_ports: registration.implemented_ports,
            capabilities: registration.capabilities,
            trust_state: registration.trust_state,
            conformance_state: registration.conformance_state,
            created_at_ms: registration.created_at_ms,
        };
        insert_unique(
            &mut lock(&self.state)?.registrations,
            (
                row.adapter_id,
                row.version.clone(),
                row.bundle_digest.clone(),
            ),
            row,
            "duplicate adapter registration",
        )
    }

    async fn set_conformance_state(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
        state: domain::security::ConformanceState,
    ) -> errors::Result<()> {
        let mut s = lock(&self.state)?;
        let key = (adapter_id, version.to_owned(), bundle_digest.to_owned());
        let Some(row) = s.registrations.get_mut(&key) else {
            return Err(not_found("adapter registration"));
        };
        row.conformance_state = state;
        Ok(())
    }

    async fn insert_instance(&mut self, instance: NewAdapterInstance) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "adapter_instances.state",
            &instance.state,
            &["starting", "ready", "exited", "failed"],
        )?;
        let key = (
            instance.adapter_id,
            instance.adapter_version.clone(),
            instance.bundle_digest.clone(),
        );
        if !state.registrations.contains_key(&key) {
            return Err(failed_precondition(
                "adapter instance references a missing registration",
            ));
        }
        let row = AdapterInstanceRow {
            adapter_instance_id: instance.adapter_instance_id,
            adapter_id: instance.adapter_id,
            adapter_version: instance.adapter_version,
            bundle_digest: instance.bundle_digest,
            daemon_instance_id: instance.daemon_instance_id,
            pid: instance.pid,
            process_start_identity: instance.process_start_identity,
            state: instance.state,
            exit_reason: instance.exit_reason,
            last_heartbeat_ms: instance.last_heartbeat_ms,
            started_at_ms: instance.started_at_ms,
            ended_at_ms: instance.ended_at_ms,
        };
        insert_unique(
            &mut state.instances,
            row.adapter_instance_id,
            row,
            "duplicate adapter instance id",
        )
    }

    async fn cas_instance_state(
        &mut self,
        id: AdapterInstanceId,
        expect_state: &str,
        patch: AdapterInstanceStatePatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(instance) = state.instances.get(&id) else {
            return Ok(false);
        };
        if instance.state != expect_state {
            return Ok(false);
        }
        if let Some(value) = &patch.state {
            check_literal(
                "adapter_instances.state",
                value,
                &["starting", "ready", "exited", "failed"],
            )?;
        }
        let instance = state
            .instances
            .get_mut(&id)
            .expect("instance checked above");
        if let Some(value) = patch.state {
            instance.state = value;
        }
        if let Some(value) = patch.exit_reason {
            instance.exit_reason = Some(value);
        }
        if let Some(value) = patch.last_heartbeat_ms {
            instance.last_heartbeat_ms = Some(value);
        }
        if let Some(value) = patch.ended_at_ms {
            instance.ended_at_ms = Some(value);
        }
        Ok(true)
    }

    async fn insert_conformance_report(
        &mut self,
        report: NewConformanceReport,
    ) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "conformance_reports.result",
            &report.result,
            &["pass", "fail"],
        )?;
        let key = (
            report.adapter_id,
            report.adapter_version.clone(),
            report.bundle_digest.clone(),
        );
        if !state.registrations.contains_key(&key) {
            return Err(failed_precondition(
                "conformance report references a missing registration",
            ));
        }
        let row = ConformanceReportRow {
            adapter_id: report.adapter_id,
            adapter_version: report.adapter_version,
            bundle_digest: report.bundle_digest,
            report_digest: report.report_digest,
            harness_version: report.harness_version,
            result: report.result,
            run_at_ms: report.run_at_ms,
            details: report.details,
        };
        insert_unique(
            &mut state.conformance_reports,
            (
                row.adapter_id,
                row.adapter_version.clone(),
                row.bundle_digest.clone(),
            ),
            row,
            "duplicate conformance report",
        )
    }
}

#[async_trait]
impl ArtifactRead for MockArtifactRepo {
    async fn get_by_id(&mut self, id: ArtifactId) -> errors::Result<Option<ArtifactRow>> {
        Ok(lock(&self.state)?.artifacts_by_id.get(&id).cloned())
    }

    async fn get_by_uri(&mut self, uri: &str) -> errors::Result<Option<ArtifactRow>> {
        let state = lock(&self.state)?;
        Ok(state
            .artifacts_by_uri
            .get(uri)
            .and_then(|id| state.artifacts_by_id.get(id))
            .cloned())
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<ArtifactRow>> {
        let mut rows: Vec<_> = lock(&self.state)?
            .artifacts_by_id
            .values()
            .filter(|row| row.origin_run_id == run)
            .cloned()
            .collect();
        rows.sort_by_key(|row| row.created_at_ms);
        Ok(rows)
    }
}

#[async_trait]
impl ArtifactRepo for MockArtifactRepo {
    async fn insert(&mut self, artifact: NewArtifact) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if !state.runs.contains_key(&artifact.origin_run_id) {
            return Err(failed_precondition("artifact references a missing run"));
        }
        if let Some(effect) = artifact.origin_effect_id
            && !state.effects.contains_key(&effect)
        {
            return Err(failed_precondition("artifact references a missing effect"));
        }
        if state.artifacts_by_uri.contains_key(&artifact.uri) {
            return Err(conflict("duplicate artifact uri"));
        }
        let row = ArtifactRow {
            artifact_id: artifact.artifact_id,
            uri: artifact.uri,
            digest: artifact.digest,
            media_type: artifact.media_type,
            size_bytes: artifact.size_bytes,
            origin_run_id: artifact.origin_run_id,
            origin_effect_id: artifact.origin_effect_id,
            sensitivity: artifact.sensitivity,
            retention: artifact.retention,
            locator: artifact.locator,
            created_at_ms: artifact.created_at_ms,
        };
        state
            .artifacts_by_uri
            .insert(row.uri.clone(), row.artifact_id);
        insert_unique(
            &mut state.artifacts_by_id,
            row.artifact_id,
            row,
            "duplicate artifact id",
        )
    }
}

#[async_trait]
impl LoopRead for MockLoopRepo {
    async fn get_turn(&mut self, id: TurnId) -> errors::Result<Option<LoopTurnRow>> {
        Ok(lock(&self.state)?.turns.get(&id).cloned())
    }

    async fn get_decision(
        &mut self,
        run: RunId,
        decision: DecisionId,
    ) -> errors::Result<Option<DecisionRow>> {
        Ok(lock(&self.state)?.decisions.get(&(run, decision)).cloned())
    }
}

#[async_trait]
impl LoopRepo for MockLoopRepo {
    async fn insert_turn(&mut self, turn: NewLoopTurn) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "loop_turns.state",
            &turn.state,
            &["issued", "accepted", "stale"],
        )?;
        if !state.runs.contains_key(&turn.run_id) {
            return Err(failed_precondition("turn references a missing run"));
        }
        let row = LoopTurnRow {
            turn_id: turn.turn_id,
            run_id: turn.run_id,
            run_revision: turn.run_revision,
            loop_epoch: turn.loop_epoch,
            step_sequence: turn.step_sequence,
            input_event_cursor: turn.input_event_cursor,
            state: turn.state,
            issued_at_ms: turn.issued_at_ms,
        };
        insert_unique(&mut state.turns, row.turn_id, row, "duplicate turn id")
    }

    async fn cas_turn(
        &mut self,
        id: TurnId,
        expect_state: &str,
        patch: LoopTurnPatch,
    ) -> errors::Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(turn) = state.turns.get(&id) else {
            return Ok(false);
        };
        if turn.state != expect_state {
            return Ok(false);
        }
        if let Some(value) = &patch.state {
            check_literal("loop_turns.state", value, &["issued", "accepted", "stale"])?;
        }
        let turn = state.turns.get_mut(&id).expect("turn checked above");
        if let Some(value) = patch.state {
            turn.state = value;
        }
        Ok(true)
    }

    async fn insert_decision(&mut self, decision: NewDecision) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        check_literal(
            "decisions.decision_type",
            &decision.decision_type,
            &[
                "Complete",
                "Fail",
                "Wait",
                "SpawnAgent",
                "InvokeEffect",
                "RequestApproval",
            ],
        )?;
        if !state.runs.contains_key(&decision.run_id) {
            return Err(failed_precondition("decision references a missing run"));
        }
        if !state.turns.contains_key(&decision.turn_id) {
            return Err(failed_precondition("decision references a missing turn"));
        }
        if state
            .decisions
            .contains_key(&(decision.run_id, decision.decision_id))
        {
            return Err(conflict("duplicate decision for run"));
        }
        let row = DecisionRow {
            decision_id: decision.decision_id,
            run_id: decision.run_id,
            turn_id: decision.turn_id,
            decision_type: decision.decision_type,
            decision_digest: decision.decision_digest,
            decision_bytes: decision.decision_bytes,
            run_revision: decision.run_revision,
            loop_epoch: decision.loop_epoch,
            step_sequence: decision.step_sequence,
            input_event_cursor: decision.input_event_cursor,
            accepted_at_ms: decision.accepted_at_ms,
        };
        insert_unique(
            &mut state.decisions,
            (row.run_id, row.decision_id),
            row,
            "duplicate decision id",
        )
    }
}

#[async_trait]
impl IdempotencyRepo for MockIdempotencyRepo {
    async fn lookup(
        &mut self,
        principal: PrincipalId,
        key: &IdempotencyKey,
    ) -> errors::Result<Option<IdempotencyRecordRow>> {
        Ok(lock(&self.state)?
            .idempotency
            .get(&(principal, key.clone()))
            .cloned())
    }

    async fn insert(&mut self, record: NewIdempotencyRecord) -> errors::Result<()> {
        let row = IdempotencyRecordRow {
            principal_id: record.principal_id,
            idempotency_key: record.idempotency_key,
            request_digest: record.request_digest,
            command_id: record.command_id,
            outcome_code: record.outcome_code,
            outcome_payload: record.outcome_payload,
            created_at_ms: record.created_at_ms,
        };
        let key = (row.principal_id, row.idempotency_key.clone());
        insert_unique(
            &mut lock(&self.state)?.idempotency,
            key,
            row,
            "idempotency key already recorded",
        )
    }
}

#[async_trait]
impl StreamRepo for MockStreamRepo {
    async fn allocate(&mut self, stream_key: EventStreamKey) -> errors::Result<u64> {
        let mut state = lock(&self.state)?;
        let head = state.stream_heads.entry(stream_key).or_insert(0);
        *head = head.saturating_add(1);
        Ok(*head)
    }

    async fn insert_outbox(&mut self, event: NewOutboxEvent) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        if state.outbox.contains_key(&event.event_id) {
            return Err(conflict("duplicate outbox event id"));
        }
        if !state
            .outbox_sequences
            .insert((event.stream_key.clone(), event.sequence))
        {
            return Err(conflict("duplicate stream sequence"));
        }
        let row = OutboxEventRow {
            event_id: event.event_id,
            event_type: event.event_type,
            event_version: event.event_version,
            stream_key: event.stream_key,
            sequence: event.sequence,
            occurred_at_ms: event.occurred_at_ms,
            run_id: event.run_id,
            task_id: event.task_id,
            session_id: event.session_id,
            effect_id: event.effect_id,
            causation_id: event.causation_id,
            correlation_id: event.correlation_id,
            sensitivity: event.sensitivity,
            retention: event.retention,
            payload: event.payload,
            journal_published_at_ms: None,
            live_published_at_ms: None,
        };
        state.outbox.insert(row.event_id, row);
        Ok(())
    }

    async fn scan_unpublished(&mut self, limit: u32) -> errors::Result<Vec<OutboxEventRow>> {
        let state = lock(&self.state)?;
        let mut rows: Vec<OutboxEventRow> = state
            .outbox
            .values()
            .filter(|row| row.journal_published_at_ms.is_none())
            .cloned()
            .collect();
        rows.sort_by(|left, right| {
            left.stream_key
                .cmp(&right.stream_key)
                .then(left.sequence.cmp(&right.sequence))
        });
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(rows)
    }

    async fn mark_published(&mut self, event_id: EventId, kind: PublishKind) -> errors::Result<()> {
        let mut state = lock(&self.state)?;
        let now = state.logical_now_ms.saturating_add(1);
        state.logical_now_ms = now;
        let Some(event) = state.outbox.get_mut(&event_id) else {
            return Err(not_found("outbox event not found"));
        };
        match kind {
            PublishKind::Journal => event.journal_published_at_ms = Some(now),
            PublishKind::Live => event.live_published_at_ms = Some(now),
        }
        Ok(())
    }
}
