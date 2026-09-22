//! Row mirrors, insert models, and compare-and-set patch types for the store port.
//!
//! Every row type is a field-for-field mirror of the inception DDL
//! (`agent-os/schema/kernel_store.sql`): nullability matches, domain ids and
//! enums replace raw columns, integer enums decode through `from_wire`, TEXT
//! state columns decode through `from_state_str`, and timestamps are UTC Unix
//! milliseconds. No SQL type crosses this boundary.

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{
    ActorId, AdapterId, AdapterInstanceId, AgentSpecId, ApprovalRequestId, ArtifactId,
    CapabilityGrantId, CommandId, ConfigGenerationId, DecisionId, DelegationChainId, DependencyId,
    DeviceId, EffectId, EnvironmentId, EventCursor, EventId, EventStreamKey, IdempotencyKey,
    LeaseId, PrincipalId, ReservationId, RunId, SessionId, TaskId, TimerId, TurnId, WorkspaceId,
};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{
    ApprovalState, ConformanceState, RetentionClass, SensitivityClass, TrustState,
};

/// Row mirror of `agent_specs` (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpecRow {
    pub agent_spec_id: AgentSpecId,
    pub version: String,
    pub digest: String,
    pub body: Vec<u8>,
    pub created_at_ms: i64,
}

/// Insert model for `agent_specs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAgentSpec {
    pub agent_spec_id: AgentSpecId,
    pub version: String,
    pub digest: String,
    pub body: Vec<u8>,
    pub created_at_ms: i64,
}

/// Row mirror of `sessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub session_id: SessionId,
    pub principal_id: PrincipalId,
    pub created_at_ms: i64,
    pub metadata: Option<Vec<u8>>,
}

/// Insert model for `sessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSession {
    pub session_id: SessionId,
    pub principal_id: PrincipalId,
    pub created_at_ms: i64,
    pub metadata: Option<Vec<u8>>,
}

/// Row mirror of `tasks`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub created_by_actor_id: ActorId,
    pub task_kind: String,
    pub payload: Vec<u8>,
    pub created_at_ms: i64,
}

/// Insert model for `tasks`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTask {
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub created_by_actor_id: ActorId,
    pub task_kind: String,
    pub payload: Vec<u8>,
    pub created_at_ms: i64,
}

/// Row mirror of `resolved_run_environments` (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedEnvironmentRow {
    pub environment_id: EnvironmentId,
    pub run_id: RunId,
    pub agent_spec_id: AgentSpecId,
    pub agent_spec_version: String,
    pub agent_spec_digest: String,
    pub agent_loop_id: String,
    pub agent_loop_version: String,
    pub agent_loop_digest: String,
    pub config_generation_id: ConfigGenerationId,
    pub workspace_uri: Option<String>,
    pub workspace_base_revision: Option<String>,
    pub workspace_mode: WorkspaceAccessMode,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub model_parameters: Option<Vec<u8>>,
    pub kernel_version: String,
    pub protocol_versions: Vec<u8>,
    pub capability_grant_ids: Vec<u8>,
    pub approval_request_ids: Vec<u8>,
    pub created_at_ms: i64,
}

/// Insert model for `resolved_run_environments`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewResolvedEnvironment {
    pub environment_id: EnvironmentId,
    pub run_id: RunId,
    pub agent_spec_id: AgentSpecId,
    pub agent_spec_version: String,
    pub agent_spec_digest: String,
    pub agent_loop_id: String,
    pub agent_loop_version: String,
    pub agent_loop_digest: String,
    pub config_generation_id: ConfigGenerationId,
    pub workspace_uri: Option<String>,
    pub workspace_base_revision: Option<String>,
    pub workspace_mode: WorkspaceAccessMode,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub model_parameters: Option<Vec<u8>>,
    pub kernel_version: String,
    pub protocol_versions: Vec<u8>,
    pub capability_grant_ids: Vec<u8>,
    pub approval_request_ids: Vec<u8>,
    pub created_at_ms: i64,
}

/// Row mirror of `resolved_bindings` (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBindingRow {
    pub environment_id: EnvironmentId,
    pub port_id: String,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub adapter_digest: String,
    pub capabilities: Vec<u8>,
}

/// Insert model for `resolved_bindings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewResolvedBinding {
    pub port_id: String,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub adapter_digest: String,
    pub capabilities: Vec<u8>,
}

/// Row mirror of `run_graph_heads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunGraphHeadRow {
    pub task_id: TaskId,
    pub graph_revision: u64,
}

/// Row mirror of `run_dependencies`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDependencyRow {
    pub dependency_id: DependencyId,
    pub task_id: TaskId,
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub dependency_condition: DependencyCondition,
    pub created_graph_revision: u64,
    pub created_at_ms: i64,
}

/// Insert model for `run_dependencies`; the graph revision is supplied by the
/// enclosing `GraphRepo` operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRunDependency {
    pub dependency_id: DependencyId,
    pub task_id: TaskId,
    pub source_run_id: RunId,
    pub target_run_id: RunId,
    pub dependency_condition: DependencyCondition,
    pub created_at_ms: i64,
}

/// Row mirror of `runs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub parent_run_id: Option<RunId>,
    pub state: RunState,
    pub recovery: RecoveryDisposition,
    pub run_revision: u64,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    pub cancellation_epoch: u64,
    pub resolved_environment_id: Option<EnvironmentId>,
    /// Binding input captured at creation: AgentSpec id reference.
    pub agent_spec_id: Option<AgentSpecId>,
    /// Binding input captured at creation: AgentSpec version.
    pub agent_spec_version: Option<String>,
    /// Binding input captured at creation: AgentSpec content digest.
    pub agent_spec_digest: Option<String>,
    /// Binding input captured at creation: requested execution profile.
    pub requested_profile: String,
    /// Binding input captured at creation: pre-provisioned workspace URI.
    pub workspace_uri: Option<String>,
    pub claim_owner: Option<String>,
    pub claim_token: Option<u64>,
    pub claim_expires_ms: Option<i64>,
    pub claim_daemon_epoch: Option<u64>,
    pub terminal_reason: Option<String>,
    pub output_ref: Option<String>,
    pub current_turn_id: Option<TurnId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Insert model for `runs`; revision, claim, and terminal fields default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRun {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub session_id: Option<SessionId>,
    pub parent_run_id: Option<RunId>,
    pub state: RunState,
    pub recovery: RecoveryDisposition,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    pub cancellation_epoch: u64,
    pub resolved_environment_id: Option<EnvironmentId>,
    /// Binding inputs captured at creation (immutable).
    pub agent_spec_id: Option<AgentSpecId>,
    /// See [`RunRow::agent_spec_version`].
    pub agent_spec_version: Option<String>,
    /// See [`RunRow::agent_spec_digest`].
    pub agent_spec_digest: Option<String>,
    /// See [`RunRow::requested_profile`].
    pub requested_profile: String,
    /// See [`RunRow::workspace_uri`].
    pub workspace_uri: Option<String>,
    pub created_at_ms: i64,
}

/// CAS expectation for `runs`; `None` fields are not checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunCas {
    pub run_revision: u64,
    pub state: Option<RunState>,
    pub cancellation_epoch: Option<u64>,
}

/// Optimistic patch applied to `runs` after a successful CAS check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPatch {
    pub state: Option<RunState>,
    pub recovery: Option<RecoveryDisposition>,
    pub loop_epoch: Option<u64>,
    pub step_sequence: Option<u64>,
    pub input_event_cursor: Option<EventCursor>,
    pub cancellation_epoch: Option<u64>,
    pub resolved_environment_id: Option<EnvironmentId>,
    pub claim: Option<ClaimPatch>,
    pub terminal_reason: Option<String>,
    pub output_ref: Option<String>,
    pub current_turn_id: Option<TurnId>,
    pub bump_revision: bool,
}

/// Claim columns written together by a run CAS patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimPatch {
    pub owner: String,
    pub token: u64,
    pub expires_unix_ms: i64,
    pub daemon_epoch: u64,
}

/// Row mirror of `workspaces`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub workspace_id: WorkspaceId,
    pub kind: String,
    pub base_revision: Option<String>,
    pub parent_workspace_id: Option<WorkspaceId>,
    pub created_at_ms: i64,
}

/// Insert model for `workspaces`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorkspace {
    pub workspace_id: WorkspaceId,
    pub kind: String,
    pub base_revision: Option<String>,
    pub parent_workspace_id: Option<WorkspaceId>,
    pub created_at_ms: i64,
}

/// Row mirror of `workspace_leases`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLeaseRow {
    pub lease_id: LeaseId,
    pub workspace_id: WorkspaceId,
    pub owner_run_id: RunId,
    pub mode: WorkspaceAccessMode,
    pub lease_epoch: u64,
    pub enforcement_state: LeaseEnforcementState,
    pub delegated_from: Vec<u8>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Insert model for `workspace_leases`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorkspaceLease {
    pub lease_id: LeaseId,
    pub workspace_id: WorkspaceId,
    pub owner_run_id: RunId,
    pub mode: WorkspaceAccessMode,
    pub lease_epoch: u64,
    pub enforcement_state: LeaseEnforcementState,
    pub delegated_from: Vec<u8>,
    pub created_at_ms: i64,
}

/// Optimistic patch applied to a lease after a successful epoch CAS check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeasePatch {
    pub enforcement_state: Option<LeaseEnforcementState>,
    pub owner_run_id: Option<RunId>,
    pub mode: Option<WorkspaceAccessMode>,
    pub delegated_from: Option<Vec<u8>>,
    /// Next epoch to stamp on transfer; `cas_lease` still checks the
    /// current epoch, so a stale-token write to the new owner fails.
    pub lease_epoch: Option<u64>,
}

/// Row mirror of `effects`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectRow {
    pub effect_id: EffectId,
    pub run_id: RunId,
    pub step_sequence: u64,
    pub decision_id: DecisionId,
    pub operation: String,
    pub request_hash: String,
    pub request_payload: Vec<u8>,
    pub effect_class: EffectClass,
    pub idempotency_semantics: IdempotencySemantics,
    pub reconciliation_semantics: ReconciliationSemantics,
    pub cancellation_semantics: String,
    pub compensation_capability: Option<String>,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub adapter_digest: String,
    pub state: EffectState,
    pub executor_id: Option<String>,
    pub executor_fencing_token: Option<u64>,
    pub daemon_fencing_epoch: Option<u64>,
    pub lease_expires_ms: Option<i64>,
    pub provider_operation_ref: Option<String>,
    pub result_ref: Option<String>,
    pub error_code: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Insert model for `effects`; executor and result columns start null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEffect {
    pub effect_id: EffectId,
    pub run_id: RunId,
    pub step_sequence: u64,
    pub decision_id: DecisionId,
    pub operation: String,
    pub request_hash: String,
    pub request_payload: Vec<u8>,
    pub effect_class: EffectClass,
    pub idempotency_semantics: IdempotencySemantics,
    pub reconciliation_semantics: ReconciliationSemantics,
    pub cancellation_semantics: String,
    pub compensation_capability: Option<String>,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub adapter_digest: String,
    pub state: EffectState,
    pub created_at_ms: i64,
}

/// Optimistic patch applied to an effect after a successful transition check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectPatch {
    pub state: Option<EffectState>,
    pub executor_id: Option<String>,
    pub executor_fencing_token: Option<u64>,
    pub daemon_fencing_epoch: Option<u64>,
    pub lease_expires_ms: Option<i64>,
    pub provider_operation_ref: Option<String>,
    pub result_ref: Option<String>,
    pub error_code: Option<String>,
}

/// Row mirror of `resource_reservations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReservationRow {
    pub reservation_id: ReservationId,
    pub run_id: RunId,
    pub resource_type: String,
    pub state: ReservationState,
    pub amount: i64,
    pub unit: String,
    pub fencing_token: u64,
    pub parent_reservation_id: Option<ReservationId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Insert model for `resource_reservations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewReservation {
    pub reservation_id: ReservationId,
    pub run_id: RunId,
    pub resource_type: String,
    pub state: ReservationState,
    pub amount: i64,
    pub unit: String,
    pub fencing_token: u64,
    pub parent_reservation_id: Option<ReservationId>,
    pub created_at_ms: i64,
}

/// Optimistic patch applied to a reservation after a state check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReservationPatch {
    pub state: Option<ReservationState>,
    pub fencing_token: Option<u64>,
}

/// Row mirror of `timers`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerRow {
    pub timer_id: TimerId,
    pub run_id: Option<RunId>,
    pub timer_kind: String,
    pub payload: Vec<u8>,
    pub due_at_ms: i64,
    pub state: TimerState,
    pub version: u64,
    pub claim_owner: Option<String>,
    pub claim_fencing_token: Option<u64>,
    pub claim_daemon_epoch: Option<u64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Insert model for `timers`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTimer {
    pub timer_id: TimerId,
    pub run_id: Option<RunId>,
    pub timer_kind: String,
    pub payload: Vec<u8>,
    pub due_at_ms: i64,
    pub state: TimerState,
    pub version: u64,
    pub created_at_ms: i64,
}

/// Optimistic patch applied to a timer after state and version checks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerPatch {
    pub state: Option<TimerState>,
    pub claim_owner: Option<String>,
    pub claim_fencing_token: Option<u64>,
    pub claim_daemon_epoch: Option<u64>,
    pub due_at_ms: Option<i64>,
}

/// Row mirror of `loop_turns`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopTurnRow {
    pub turn_id: TurnId,
    pub run_id: RunId,
    pub run_revision: u64,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    /// Persisted literals: `issued`, `accepted`, `stale`.
    pub state: String,
    pub issued_at_ms: i64,
}

/// Insert model for `loop_turns`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewLoopTurn {
    pub turn_id: TurnId,
    pub run_id: RunId,
    pub run_revision: u64,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    pub state: String,
    pub issued_at_ms: i64,
}

/// Optimistic patch applied to a turn after a state check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoopTurnPatch {
    pub state: Option<String>,
}

/// Row mirror of `decisions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionRow {
    pub decision_id: DecisionId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    /// Canonical LoopDecision message name.
    pub decision_type: String,
    pub decision_digest: String,
    pub decision_bytes: Vec<u8>,
    pub run_revision: u64,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    pub accepted_at_ms: i64,
}

/// Insert model for `decisions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDecision {
    pub decision_id: DecisionId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub decision_type: String,
    pub decision_digest: String,
    pub decision_bytes: Vec<u8>,
    pub run_revision: u64,
    pub loop_epoch: u64,
    pub step_sequence: u64,
    pub input_event_cursor: EventCursor,
    pub accepted_at_ms: i64,
}

/// Row mirror of `capability_grants`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityGrantRow {
    pub grant_id: CapabilityGrantId,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option<RunId>,
    pub capability_id: String,
    pub scope: Vec<u8>,
    pub delegated_from_grant_id: Option<CapabilityGrantId>,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

/// Insert model for `capability_grants`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCapabilityGrant {
    pub grant_id: CapabilityGrantId,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option<RunId>,
    pub capability_id: String,
    pub scope: Vec<u8>,
    pub delegated_from_grant_id: Option<CapabilityGrantId>,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

/// Row mirror of `delegation_hops`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationHopRow {
    pub chain_id: DelegationChainId,
    pub hop_index: u32,
    pub principal_or_actor_id: String,
    pub run_id: Option<RunId>,
    pub capability_grant_ids: Vec<u8>,
}

/// Insert model for `delegation_hops`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDelegationHop {
    pub chain_id: DelegationChainId,
    pub hop_index: u32,
    pub principal_or_actor_id: String,
    pub run_id: Option<RunId>,
    pub capability_grant_ids: Vec<u8>,
}

/// Row mirror of `approval_requests` (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequestRow {
    pub request_id: ApprovalRequestId,
    pub request_digest: String,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option<RunId>,
    pub operation: String,
    pub target_resource: Option<String>,
    pub capability_ids: Vec<u8>,
    pub extension_bundle_digest: Option<String>,
    pub config_generation_digest: Option<String>,
    pub expires_at_ms: i64,
    pub nonce: String,
    pub state: ApprovalState,
    pub created_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

/// Insert model for `approval_requests`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewApprovalRequest {
    pub request_id: ApprovalRequestId,
    pub request_digest: String,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option<RunId>,
    pub operation: String,
    pub target_resource: Option<String>,
    pub capability_ids: Vec<u8>,
    pub extension_bundle_digest: Option<String>,
    pub config_generation_digest: Option<String>,
    pub expires_at_ms: i64,
    pub nonce: String,
    pub state: ApprovalState,
    pub created_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

/// Row mirror of `approval_responses`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalResponseRow {
    pub request_id: ApprovalRequestId,
    pub request_digest: String,
    /// Persisted literals: `approve`, `deny`.
    pub decision: String,
    pub device_id: DeviceId,
    pub responder_principal_id: PrincipalId,
    pub responded_at_ms: i64,
}

/// Insert model for `approval_responses`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewApprovalResponse {
    pub request_id: ApprovalRequestId,
    pub request_digest: String,
    pub decision: String,
    pub device_id: DeviceId,
    pub responder_principal_id: PrincipalId,
    pub responded_at_ms: i64,
}

/// Row mirror of `adapter_registrations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterRegistrationRow {
    pub adapter_id: AdapterId,
    pub version: String,
    pub bundle_digest: String,
    pub manifest_digest: String,
    pub runtime_type: String,
    pub implemented_ports: Vec<u8>,
    pub capabilities: Vec<u8>,
    pub trust_state: TrustState,
    pub conformance_state: ConformanceState,
    pub created_at_ms: i64,
}

/// Insert model for `adapter_registrations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAdapterRegistration {
    pub adapter_id: AdapterId,
    pub version: String,
    pub bundle_digest: String,
    pub manifest_digest: String,
    pub runtime_type: String,
    pub implemented_ports: Vec<u8>,
    pub capabilities: Vec<u8>,
    pub trust_state: TrustState,
    pub conformance_state: ConformanceState,
    pub created_at_ms: i64,
}

/// Row mirror of `adapter_instances`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInstanceRow {
    pub adapter_instance_id: AdapterInstanceId,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub bundle_digest: String,
    pub daemon_instance_id: domain::ids::DaemonInstanceId,
    pub pid: Option<i64>,
    pub process_start_identity: Option<String>,
    /// Persisted literals: `starting`, `ready`, `exited`, `failed`.
    pub state: String,
    pub exit_reason: Option<String>,
    pub last_heartbeat_ms: Option<i64>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

/// Insert model for `adapter_instances`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAdapterInstance {
    pub adapter_instance_id: AdapterInstanceId,
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub bundle_digest: String,
    pub daemon_instance_id: domain::ids::DaemonInstanceId,
    pub pid: Option<i64>,
    pub process_start_identity: Option<String>,
    pub state: String,
    pub exit_reason: Option<String>,
    pub last_heartbeat_ms: Option<i64>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

/// Optimistic patch applied to an adapter instance after a state check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterInstanceStatePatch {
    pub state: Option<String>,
    pub exit_reason: Option<String>,
    pub last_heartbeat_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

/// Row mirror of `conformance_reports` (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceReportRow {
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub bundle_digest: String,
    pub report_digest: String,
    pub harness_version: String,
    /// Persisted literals: `pass`, `fail`.
    pub result: String,
    pub run_at_ms: i64,
    pub details: Option<Vec<u8>>,
}

/// Insert model for `conformance_reports`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewConformanceReport {
    pub adapter_id: AdapterId,
    pub adapter_version: String,
    pub bundle_digest: String,
    pub report_digest: String,
    pub harness_version: String,
    pub result: String,
    pub run_at_ms: i64,
    pub details: Option<Vec<u8>>,
}

/// Row mirror of `artifacts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRow {
    pub artifact_id: ArtifactId,
    pub uri: String,
    pub digest: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub origin_run_id: RunId,
    pub origin_effect_id: Option<EffectId>,
    pub sensitivity: SensitivityClass,
    pub retention: RetentionClass,
    pub locator: String,
    pub created_at_ms: i64,
}

/// Insert model for `artifacts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewArtifact {
    pub artifact_id: ArtifactId,
    pub uri: String,
    pub digest: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub origin_run_id: RunId,
    pub origin_effect_id: Option<EffectId>,
    pub sensitivity: SensitivityClass,
    pub retention: RetentionClass,
    pub locator: String,
    pub created_at_ms: i64,
}

/// Row mirror of `config_generations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigGenerationRow {
    pub generation_id: ConfigGenerationId,
    pub digest: String,
    pub document: Vec<u8>,
    /// Persisted literals: `proposed`, `validated`, `rejected`.
    pub validation_state: String,
    /// Persisted literals: `untested`, `passed`, `failed`.
    pub test_state: String,
    pub created_by_actor_id: ActorId,
    pub created_at_ms: i64,
}

/// Insert model for `config_generations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewConfigGeneration {
    pub generation_id: ConfigGenerationId,
    pub digest: String,
    pub document: Vec<u8>,
    pub validation_state: String,
    pub test_state: String,
    pub created_by_actor_id: ActorId,
    pub created_at_ms: i64,
}

/// Row mirror of `active_config_generation` (singleton).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveConfigGenerationRow {
    pub generation_id: ConfigGenerationId,
    pub revision: u64,
    pub activated_at_ms: i64,
}

/// Row mirror of `idempotency_records`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyRecordRow {
    pub principal_id: PrincipalId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: String,
    pub command_id: CommandId,
    pub outcome_code: String,
    pub outcome_payload: Vec<u8>,
    pub created_at_ms: i64,
}

/// Insert model for `idempotency_records`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIdempotencyRecord {
    pub principal_id: PrincipalId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: String,
    pub command_id: CommandId,
    pub outcome_code: String,
    pub outcome_payload: Vec<u8>,
    pub created_at_ms: i64,
}

/// Row mirror of `event_stream_heads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventStreamHeadRow {
    pub stream_key: EventStreamKey,
    pub last_sequence: u64,
}

/// Row mirror of `outbox_events`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxEventRow {
    pub event_id: EventId,
    pub event_type: String,
    pub event_version: u32,
    pub stream_key: EventStreamKey,
    pub sequence: u64,
    pub occurred_at_ms: i64,
    pub run_id: Option<RunId>,
    pub task_id: Option<TaskId>,
    pub session_id: Option<SessionId>,
    pub effect_id: Option<EffectId>,
    pub causation_id: Option<EventId>,
    pub correlation_id: Option<String>,
    pub sensitivity: SensitivityClass,
    pub retention: RetentionClass,
    pub payload: Vec<u8>,
    pub journal_published_at_ms: Option<i64>,
    pub live_published_at_ms: Option<i64>,
}

/// Insert model for `outbox_events`; publication columns start null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewOutboxEvent {
    pub event_id: EventId,
    pub event_type: String,
    pub event_version: u32,
    pub stream_key: EventStreamKey,
    pub sequence: u64,
    pub occurred_at_ms: i64,
    pub run_id: Option<RunId>,
    pub task_id: Option<TaskId>,
    pub session_id: Option<SessionId>,
    pub effect_id: Option<EffectId>,
    pub causation_id: Option<EventId>,
    pub correlation_id: Option<String>,
    pub sensitivity: SensitivityClass,
    pub retention: RetentionClass,
    pub payload: Vec<u8>,
}
