//! Repository traits.
//!
//! Every write trait extends its read trait; read traits expose no mutation
//! (D3) so a read transaction cannot be given a mutating repository. All row,
//! insert, and patch types live in [`crate::models`] (D4).

use async_trait::async_trait;
use domain::effect::EffectState;
use domain::ids::{
    AdapterId, AdapterInstanceId, ApprovalRequestId, ArtifactId, CapabilityGrantId,
    ConfigGenerationId, DecisionId, DelegationChainId, EffectId, EnvironmentId, EventId,
    EventStreamKey, IdempotencyKey, LeaseId, PrincipalId, ReservationId, RunId, SessionId, TaskId,
    TimerId, TurnId, WorkspaceId,
};
use domain::resource::{ReservationState, TimerState};
use errors::Result;

use crate::models::{
    ActiveConfigGenerationRow, AdapterInstanceStatePatch, AdapterRegistrationRow,
    ApprovalRequestRow, ApprovalResponseRow, ArtifactRow, CapabilityGrantRow, ConfigGenerationRow,
    ConformanceReportRow, DecisionRow, DelegationHopRow, EffectPatch, EffectRow,
    IdempotencyRecordRow, LeasePatch, LoopTurnPatch, LoopTurnRow, NewAdapterInstance,
    NewAdapterRegistration, NewApprovalRequest, NewApprovalResponse, NewArtifact,
    NewCapabilityGrant, NewConfigGeneration, NewConformanceReport, NewDecision, NewDelegationHop,
    NewEffect, NewIdempotencyRecord, NewLoopTurn, NewOutboxEvent, NewReservation,
    NewResolvedBinding, NewResolvedEnvironment, NewRun, NewRunDependency, NewSession, NewTask,
    NewTimer, NewWorkspace, NewWorkspaceLease, OutboxEventRow, ReservationPatch, ReservationRow,
    ResolvedBindingRow, ResolvedEnvironmentRow, RunCas, RunDependencyRow, RunGraphHeadRow,
    RunPatch, RunRow, SessionRow, TaskRow, TimerPatch, TimerRow, WorkspaceLeaseRow, WorkspaceRow,
};

/// Publication phase observed by [`StreamRepo::mark_published`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublishKind {
    /// Durable journal publication.
    Journal,
    /// Live delivery publication.
    Live,
}

#[async_trait]
pub trait RunRead: Send + Sync {
    async fn get(&mut self, id: RunId) -> Result<Option<RunRow>>;
    async fn list_by_task(&mut self, task: TaskId) -> Result<Vec<RunRow>>;
}

#[async_trait]
pub trait RunRepo: RunRead {
    async fn insert(&mut self, run: NewRun) -> Result<()>;
    async fn cas_update(&mut self, id: RunId, expect: RunCas, patch: RunPatch) -> Result<bool>;
}

#[async_trait]
pub trait TaskRead: Send + Sync {
    async fn get(&mut self, id: TaskId) -> Result<Option<TaskRow>>;
    async fn list_by_session(&mut self, session: SessionId) -> Result<Vec<TaskRow>>;
}

#[async_trait]
pub trait TaskRepo: TaskRead {
    async fn insert(&mut self, task: NewTask) -> Result<()>;
}

#[async_trait]
pub trait SessionRead: Send + Sync {
    async fn get(&mut self, id: SessionId) -> Result<Option<SessionRow>>;
}

#[async_trait]
pub trait SessionRepo: SessionRead {
    async fn insert(&mut self, session: NewSession) -> Result<()>;
}

#[async_trait]
pub trait GraphRead: Send + Sync {
    async fn get_head(&mut self, task: TaskId) -> Result<Option<RunGraphHeadRow>>;
    async fn list_dependencies(&mut self, task: TaskId) -> Result<Vec<RunDependencyRow>>;
    async fn is_reachable(&mut self, from: RunId, to: RunId) -> Result<bool>;
}

#[async_trait]
pub trait GraphRepo: GraphRead {
    async fn ensure_head(&mut self, task: TaskId) -> Result<RunGraphHeadRow>;
    async fn cas_head_revision(&mut self, task: TaskId, expected: u64) -> Result<bool>;
    async fn insert_dependency(
        &mut self,
        new: NewRunDependency,
        expected_revision: u64,
    ) -> Result<()>;
}

#[async_trait]
pub trait EnvironmentRead: Send + Sync {
    async fn get_environment(
        &mut self,
        id: EnvironmentId,
    ) -> Result<Option<ResolvedEnvironmentRow>>;
    async fn get_bindings(&mut self, id: EnvironmentId) -> Result<Vec<ResolvedBindingRow>>;
}

#[async_trait]
pub trait EnvironmentRepo: EnvironmentRead {
    async fn insert_environment(&mut self, new: NewResolvedEnvironment) -> Result<()>;
    async fn insert_bindings(
        &mut self,
        environment: EnvironmentId,
        bindings: Vec<NewResolvedBinding>,
    ) -> Result<()>;
}

#[async_trait]
pub trait EffectRead: Send + Sync {
    async fn get(&mut self, id: EffectId) -> Result<Option<EffectRow>>;
    async fn list_by_run(&mut self, run: RunId) -> Result<Vec<EffectRow>>;
}

#[async_trait]
pub trait EffectRepo: EffectRead {
    async fn insert(&mut self, effect: NewEffect) -> Result<()>;
    async fn cas_transition(
        &mut self,
        id: EffectId,
        expect_state: EffectState,
        expect_token: Option<u64>,
        patch: EffectPatch,
    ) -> Result<bool>;
}

#[async_trait]
pub trait ResourceRead: Send + Sync {
    async fn get(&mut self, id: ReservationId) -> Result<Option<ReservationRow>>;
    async fn list_by_run(&mut self, run: RunId) -> Result<Vec<ReservationRow>>;
}

#[async_trait]
pub trait ResourceRepo: ResourceRead {
    async fn insert(&mut self, reservation: NewReservation) -> Result<()>;
    async fn cas_transition(
        &mut self,
        id: ReservationId,
        expect_state: ReservationState,
        patch: ReservationPatch,
    ) -> Result<bool>;
}

#[async_trait]
pub trait TimerRead: Send + Sync {
    async fn get(&mut self, id: TimerId) -> Result<Option<TimerRow>>;
    async fn list_due(&mut self, due_before_ms: i64) -> Result<Vec<TimerRow>>;
}

#[async_trait]
pub trait TimerRepo: TimerRead {
    async fn insert(&mut self, timer: NewTimer) -> Result<()>;
    async fn cas_transition(
        &mut self,
        id: TimerId,
        expect_state: TimerState,
        expect_version: u64,
        patch: TimerPatch,
    ) -> Result<bool>;
}

#[async_trait]
pub trait SecurityRead: Send + Sync {
    async fn get_grant(&mut self, id: CapabilityGrantId) -> Result<Option<CapabilityGrantRow>>;
    async fn list_delegation_hops(
        &mut self,
        chain: DelegationChainId,
    ) -> Result<Vec<DelegationHopRow>>;
    async fn get_approval_request(
        &mut self,
        id: ApprovalRequestId,
    ) -> Result<Option<ApprovalRequestRow>>;
    async fn list_approval_responses(
        &mut self,
        request: ApprovalRequestId,
    ) -> Result<Vec<ApprovalResponseRow>>;
}

#[async_trait]
pub trait SecurityRepo: SecurityRead {
    async fn insert_grant(&mut self, grant: NewCapabilityGrant) -> Result<()>;
    async fn insert_delegation_hop(&mut self, hop: NewDelegationHop) -> Result<()>;
    async fn insert_approval_request(&mut self, request: NewApprovalRequest) -> Result<()>;
    async fn insert_approval_response(&mut self, response: NewApprovalResponse) -> Result<()>;
}

#[async_trait]
pub trait ConfigRead: Send + Sync {
    async fn get_generation(
        &mut self,
        id: ConfigGenerationId,
    ) -> Result<Option<ConfigGenerationRow>>;
    async fn get_active(&mut self) -> Result<Option<ActiveConfigGenerationRow>>;
}

#[async_trait]
pub trait ConfigRepo: ConfigRead {
    async fn insert_generation(&mut self, generation: NewConfigGeneration) -> Result<()>;
    async fn cas_active(
        &mut self,
        expected_revision: u64,
        generation: ConfigGenerationId,
        activated_at_ms: i64,
    ) -> Result<bool>;
}

#[async_trait]
pub trait WorkspaceRead: Send + Sync {
    async fn get_workspace(&mut self, id: WorkspaceId) -> Result<Option<WorkspaceRow>>;
    async fn get_lease(&mut self, id: LeaseId) -> Result<Option<WorkspaceLeaseRow>>;
}

#[async_trait]
pub trait WorkspaceRepo: WorkspaceRead {
    async fn insert_workspace(&mut self, workspace: NewWorkspace) -> Result<()>;
    async fn insert_lease(&mut self, lease: NewWorkspaceLease) -> Result<()>;
    async fn cas_lease(
        &mut self,
        id: LeaseId,
        expect_epoch: u64,
        patch: LeasePatch,
    ) -> Result<bool>;
}

#[async_trait]
pub trait AdapterRead: Send + Sync {
    async fn get_registration(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> Result<Option<AdapterRegistrationRow>>;
    async fn get_conformance_report(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> Result<Option<ConformanceReportRow>>;
}

#[async_trait]
pub trait AdapterRepo: AdapterRead {
    async fn insert_registration(&mut self, registration: NewAdapterRegistration) -> Result<()>;
    async fn insert_instance(&mut self, instance: NewAdapterInstance) -> Result<()>;
    async fn cas_instance_state(
        &mut self,
        id: AdapterInstanceId,
        expect_state: &str,
        patch: AdapterInstanceStatePatch,
    ) -> Result<bool>;
    async fn insert_conformance_report(&mut self, report: NewConformanceReport) -> Result<()>;
}

#[async_trait]
pub trait ArtifactRead: Send + Sync {
    async fn get_by_id(&mut self, id: ArtifactId) -> Result<Option<ArtifactRow>>;
    async fn get_by_uri(&mut self, uri: &str) -> Result<Option<ArtifactRow>>;
}

#[async_trait]
pub trait ArtifactRepo: ArtifactRead {
    async fn insert(&mut self, artifact: NewArtifact) -> Result<()>;
}

#[async_trait]
pub trait LoopRead: Send + Sync {
    async fn get_turn(&mut self, id: TurnId) -> Result<Option<LoopTurnRow>>;
    async fn get_decision(
        &mut self,
        run: RunId,
        decision: DecisionId,
    ) -> Result<Option<DecisionRow>>;
}

#[async_trait]
pub trait LoopRepo: LoopRead {
    async fn insert_turn(&mut self, turn: NewLoopTurn) -> Result<()>;
    async fn cas_turn(
        &mut self,
        id: TurnId,
        expect_state: &str,
        patch: LoopTurnPatch,
    ) -> Result<bool>;
    async fn insert_decision(&mut self, decision: NewDecision) -> Result<()>;
}

#[async_trait]
pub trait IdempotencyRepo: Send + Sync {
    async fn lookup(
        &mut self,
        principal: PrincipalId,
        key: &IdempotencyKey,
    ) -> Result<Option<IdempotencyRecordRow>>;
    async fn insert(&mut self, record: NewIdempotencyRecord) -> Result<()>;
}

#[async_trait]
pub trait StreamRepo: Send + Sync {
    /// Allocates and returns the next contiguous sequence for `stream_key`.
    async fn allocate(&mut self, stream_key: EventStreamKey) -> Result<u64>;
    async fn insert_outbox(&mut self, event: NewOutboxEvent) -> Result<()>;
    /// Returns events awaiting journal publication ordered by stream then sequence.
    async fn scan_unpublished(&mut self, limit: u32) -> Result<Vec<OutboxEventRow>>;
    async fn mark_published(&mut self, event_id: EventId, kind: PublishKind) -> Result<()>;
}
