//! CRUD and CAS coverage for the nine remaining repository groups.
//!
//! One test per group inserts, reads, and exercises one compare-and-set
//! conflict; a final test proves unknown persisted values fail decoding with
//! `Internal`/`Never`. Every test uses its own temporary database.

use std::path::Path;

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{
    ActorId, AdapterId, AdapterInstanceId, ApprovalRequestId, ArtifactId, CapabilityGrantId,
    CommandId, ConfigGenerationId, DaemonInstanceId, DecisionId, DelegationChainId, DependencyId,
    EffectId, EventCursor, EventStreamKey, LeaseId, PrincipalId, ReservationId, RunId, SessionId,
    TaskId, TimerId, TurnId, WorkspaceId,
};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{
    ApprovalState, ConformanceState, RetentionClass, SensitivityClass, TrustState,
};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{
    AdapterInstanceStatePatch, EffectPatch, LeasePatch, LoopTurnPatch, NewAdapterInstance,
    NewAdapterRegistration, NewApprovalRequest, NewApprovalResponse, NewArtifact,
    NewCapabilityGrant, NewConfigGeneration, NewConformanceReport, NewDecision, NewDelegationHop,
    NewEffect, NewLoopTurn, NewReservation, NewRun, NewRunDependency, NewSession, NewTask,
    NewTimer, NewWorkspace, NewWorkspaceLease, ReservationPatch, TimerPatch,
};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";

fn config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 8,
        busy_timeout_ms: 5_000,
    }
}

fn context(provider: &DeterministicIds, daemon_epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch,
        principal_id: PrincipalId::new(provider),
        command_id: CommandId::new(provider),
        correlation_id: None,
    }
}

async fn open_store(dir: &Path) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(config(&dir.join(DB_FILE)))
        .await
        .unwrap();
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0)
}

struct Seeded {
    task: TaskId,
    run: RunId,
}

async fn seed_run(store: &SqliteKernelStore, provider: &DeterministicIds, epoch: u64) -> Seeded {
    let session = SessionId::new(provider);
    let task = TaskId::new(provider);
    let run = RunId::new(provider);
    let mut txn = store.begin_write(context(provider, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: ActorId::new(provider),
            task_kind: "test".to_owned(),
            payload: vec![1, 2, 3],
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.runs()
        .insert(NewRun {
            run_id: run,
            task_id: task,
            session_id: Some(session),
            parent_run_id: None,
            state: RunState::Created,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(run),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    Seeded { task, run }
}

fn new_exclusive_lease(
    lease: LeaseId,
    workspace: WorkspaceId,
    owner_run: RunId,
    enforcement_state: LeaseEnforcementState,
) -> NewWorkspaceLease {
    NewWorkspaceLease {
        lease_id: lease,
        workspace_id: workspace,
        owner_run_id: owner_run,
        mode: WorkspaceAccessMode::ExclusiveWrite,
        lease_epoch: 1,
        enforcement_state,
        delegated_from: vec![],
        created_at_ms: 20,
    }
}

fn new_effect(provider: &DeterministicIds, effect: EffectId, run: RunId) -> NewEffect {
    NewEffect {
        effect_id: effect,
        run_id: run,
        step_sequence: 1,
        decision_id: DecisionId::new(provider),
        operation: "test.operation".to_owned(),
        request_hash: "hash-1".to_owned(),
        request_payload: vec![9, 9],
        effect_class: EffectClass::ExternalMutation,
        idempotency_semantics: IdempotencySemantics::IdempotencyKeySupported,
        reconciliation_semantics: ReconciliationSemantics::StatusLookup,
        cancellation_semantics: "best_effort".to_owned(),
        compensation_capability: None,
        adapter_id: AdapterId::new(provider),
        adapter_version: "1.0.0".to_owned(),
        adapter_digest: "digest-1".to_owned(),
        state: EffectState::Prepared,
        created_at_ms: 20,
    }
}

#[tokio::test]
async fn effect_repo_crud_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let effect = EffectId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.effects()
        .insert(new_effect(&provider, effect, seeded.run))
        .await
        .unwrap();
    let applied = txn
        .effects()
        .cas_transition(
            effect,
            EffectState::Prepared,
            None,
            EffectPatch {
                state: Some(EffectState::Claimed),
                executor_id: Some("executor-1".to_owned()),
                executor_fencing_token: Some(7),
                daemon_fencing_epoch: Some(epoch),
                lease_expires_ms: Some(1_800_000_000_000),
                ..EffectPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(
        applied,
        "effect transition with a matching state must apply"
    );
    let stale_state = txn
        .effects()
        .cas_transition(
            effect,
            EffectState::Prepared,
            None,
            EffectPatch {
                state: Some(EffectState::Committed),
                ..EffectPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale_state, "stale effect state must not apply");
    let stale_token = txn
        .effects()
        .cas_transition(
            effect,
            EffectState::Claimed,
            Some(8),
            EffectPatch {
                state: Some(EffectState::Committed),
                ..EffectPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale_token, "stale executor token must not apply");
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let row = read.effects().get(effect).await.unwrap().unwrap();
    assert_eq!(row.state, EffectState::Claimed);
    assert_eq!(row.executor_id.as_deref(), Some("executor-1"));
    assert_eq!(row.executor_fencing_token, Some(7));
    assert_eq!(row.daemon_fencing_epoch, Some(epoch));
    assert_eq!(row.lease_expires_ms, Some(1_800_000_000_000));
    let listed = read.effects().list_by_run(seeded.run).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].effect_id, effect);
    assert!(
        read.effects()
            .get(EffectId::new(&provider))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn resource_repo_crud_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let reservation = ReservationId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.resources()
        .insert(NewReservation {
            reservation_id: reservation,
            run_id: seeded.run,
            resource_type: "cpu".to_owned(),
            state: ReservationState::Reserved,
            amount: 2,
            unit: "cores".to_owned(),
            fencing_token: 1,
            parent_reservation_id: None,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    let applied = txn
        .resources()
        .cas_transition(
            reservation,
            ReservationState::Reserved,
            ReservationPatch {
                state: Some(ReservationState::Allocated),
                fencing_token: Some(9),
            },
        )
        .await
        .unwrap();
    assert!(applied, "reservation transition must apply");
    let stale = txn
        .resources()
        .cas_transition(
            reservation,
            ReservationState::Reserved,
            ReservationPatch {
                state: Some(ReservationState::Released),
                fencing_token: None,
            },
        )
        .await
        .unwrap();
    assert!(!stale, "stale reservation state must not apply");
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let row = read.resources().get(reservation).await.unwrap().unwrap();
    assert_eq!(row.state, ReservationState::Allocated);
    assert_eq!(row.fencing_token, 9);
    let listed = read.resources().list_by_run(seeded.run).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        read.resources()
            .get(ReservationId::new(&provider))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn timer_repo_crud_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let timer = TimerId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.timers()
        .insert(NewTimer {
            timer_id: timer,
            run_id: Some(seeded.run),
            timer_kind: "deadline".to_owned(),
            payload: vec![4, 5],
            due_at_ms: 100,
            state: TimerState::Scheduled,
            version: 0,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    let applied = txn
        .timers()
        .cas_transition(
            timer,
            TimerState::Scheduled,
            0,
            TimerPatch {
                state: Some(TimerState::Claimed),
                claim_owner: Some("scheduler-1".to_owned()),
                claim_fencing_token: Some(3),
                claim_daemon_epoch: Some(epoch),
                due_at_ms: None,
            },
        )
        .await
        .unwrap();
    assert!(applied, "timer claim must apply");
    let stale = txn
        .timers()
        .cas_transition(
            timer,
            TimerState::Scheduled,
            0,
            TimerPatch {
                state: Some(TimerState::Fired),
                ..TimerPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale, "stale timer version must not apply");
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let row = read.timers().get(timer).await.unwrap().unwrap();
    assert_eq!(row.state, TimerState::Claimed);
    assert_eq!(row.version, 1, "a successful CAS advances the version");
    assert_eq!(row.claim_owner.as_deref(), Some("scheduler-1"));
    let due = read.timers().list_due(100).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].timer_id, timer);
    assert!(read.timers().list_due(99).await.unwrap().is_empty());
    assert!(
        read.timers()
            .get(TimerId::new(&provider))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn security_repo_crud() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let grant = CapabilityGrantId::new(&provider);
    let chain = DelegationChainId::new(&provider);
    let request = ApprovalRequestId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: grant,
            principal_id: PrincipalId::new(&provider),
            actor_id: ActorId::new(&provider),
            run_id: Some(seeded.run),
            capability_id: "filesystem.write".to_owned(),
            scope: vec![1, 2],
            delegated_from_grant_id: None,
            expires_at_ms: Some(1_900_000_000_000),
            revoked_at_ms: None,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    for (index, actor) in ["principal-a", "principal-b"].iter().enumerate() {
        txn.security()
            .insert_delegation_hop(NewDelegationHop {
                chain_id: chain,
                hop_index: u32::try_from(index).unwrap(),
                principal_or_actor_id: (*actor).to_owned(),
                run_id: Some(seeded.run),
                capability_grant_ids: vec![1],
            })
            .await
            .unwrap();
    }
    txn.security()
        .insert_approval_request(NewApprovalRequest {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            principal_id: PrincipalId::new(&provider),
            actor_id: ActorId::new(&provider),
            run_id: Some(seeded.run),
            operation: "write-file".to_owned(),
            target_resource: Some("file:///tmp/x".to_owned()),
            capability_ids: vec![1],
            extension_bundle_digest: None,
            config_generation_digest: None,
            expires_at_ms: 1_900_000_000_000,
            nonce: "nonce-1".to_owned(),
            state: ApprovalState::Pending,
            created_at_ms: 20,
            resolved_at_ms: None,
        })
        .await
        .unwrap();
    txn.security()
        .insert_approval_response(NewApprovalResponse {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            decision: "approve".to_owned(),
            device_id: domain::ids::DeviceId::new(&provider),
            responder_principal_id: PrincipalId::new(&provider),
            responded_at_ms: 30,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let grant_row = read.security().get_grant(grant).await.unwrap().unwrap();
    assert_eq!(grant_row.capability_id, "filesystem.write");
    assert_eq!(grant_row.expires_at_ms, Some(1_900_000_000_000));
    let hops = read.security().list_delegation_hops(chain).await.unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].hop_index, 0);
    assert_eq!(hops[1].principal_or_actor_id, "principal-b");
    let request_row = read
        .security()
        .get_approval_request(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request_row.state, ApprovalState::Pending);
    let responses = read
        .security()
        .list_approval_responses(request)
        .await
        .unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].decision, "approve");
}

#[tokio::test]
async fn config_repo_crud_and_cas_active() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let first = ConfigGenerationId::new(&provider);
    let second = ConfigGenerationId::new(&provider);
    let missing = ConfigGenerationId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    for (id, digest, created_at_ms) in [(first, "digest-1", 20), (second, "digest-2", 21)] {
        txn.config()
            .insert_generation(NewConfigGeneration {
                generation_id: id,
                digest: digest.to_owned(),
                document: vec![7],
                validation_state: "validated".to_owned(),
                test_state: "passed".to_owned(),
                created_by_actor_id: ActorId::new(&provider),
                created_at_ms,
            })
            .await
            .unwrap();
    }
    assert!(
        txn.config().get_active().await.unwrap().is_none(),
        "no active pointer before the first CAS"
    );
    assert!(
        txn.config().cas_active(0, first, 30).await.unwrap(),
        "activating from revision zero must apply"
    );
    assert!(
        !txn.config().cas_active(0, second, 31).await.unwrap(),
        "stale active revision must not apply"
    );
    assert!(
        txn.config().cas_active(1, second, 32).await.unwrap(),
        "matching revision must apply"
    );
    let missing_error = txn.config().cas_active(2, missing, 33).await.unwrap_err();
    assert_eq!(missing_error.code(), ErrorCode::NotFound);
    assert_eq!(missing_error.retry_class(), RetryClass::Never);
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let generation = read.config().get_generation(first).await.unwrap().unwrap();
    assert_eq!(generation.digest, "digest-1");
    assert_eq!(generation.validation_state, "validated");
    let active = read.config().get_active().await.unwrap().unwrap();
    assert_eq!(active.generation_id, second);
    assert_eq!(active.revision, 2);
    assert_eq!(active.activated_at_ms, 32);
}

#[tokio::test]
async fn workspace_repo_crud_and_cas_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let workspace = WorkspaceId::new(&provider);
    let lease = LeaseId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.workspaces()
        .insert_workspace(NewWorkspace {
            workspace_id: workspace,
            kind: "git".to_owned(),
            base_revision: Some("abc123".to_owned()),
            parent_workspace_id: None,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    txn.workspaces()
        .insert_lease(NewWorkspaceLease {
            lease_id: lease,
            workspace_id: workspace,
            owner_run_id: seeded.run,
            mode: WorkspaceAccessMode::ExclusiveWrite,
            lease_epoch: 1,
            enforcement_state: LeaseEnforcementState::Active,
            delegated_from: vec![],
            created_at_ms: 20,
        })
        .await
        .unwrap();
    let applied = txn
        .workspaces()
        .cas_lease(
            lease,
            1,
            LeasePatch {
                enforcement_state: Some(LeaseEnforcementState::Revoked),
                ..LeasePatch::default()
            },
        )
        .await
        .unwrap();
    assert!(applied, "lease revocation at the current epoch must apply");
    let stale = txn
        .workspaces()
        .cas_lease(
            lease,
            0,
            LeasePatch {
                enforcement_state: Some(LeaseEnforcementState::Active),
                ..LeasePatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale, "stale lease epoch must not apply");
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let workspace_row = read
        .workspaces()
        .get_workspace(workspace)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace_row.kind, "git");
    assert_eq!(workspace_row.base_revision.as_deref(), Some("abc123"));
    let lease_row = read.workspaces().get_lease(lease).await.unwrap().unwrap();
    assert_eq!(lease_row.mode, WorkspaceAccessMode::ExclusiveWrite);
    assert_eq!(lease_row.lease_epoch, 1);
    assert_eq!(
        lease_row.enforcement_state,
        LeaseEnforcementState::Revoked,
        "only the matching CAS may mutate"
    );
}

#[tokio::test]
async fn exclusive_lease_index_rejects_second_active_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let workspace = WorkspaceId::new(&provider);
    let first = LeaseId::new(&provider);
    let second = LeaseId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.workspaces()
        .insert_workspace(NewWorkspace {
            workspace_id: workspace,
            kind: "git".to_owned(),
            base_revision: None,
            parent_workspace_id: None,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    txn.workspaces()
        .insert_lease(new_exclusive_lease(
            first,
            workspace,
            seeded.run,
            LeaseEnforcementState::Active,
        ))
        .await
        .unwrap();
    let conflict = txn
        .workspaces()
        .insert_lease(new_exclusive_lease(
            second,
            workspace,
            seeded.run,
            LeaseEnforcementState::Active,
        ))
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::Conflict);
    assert_eq!(conflict.retry_class(), RetryClass::Never);

    assert!(
        txn.workspaces()
            .cas_lease(
                first,
                1,
                LeasePatch {
                    enforcement_state: Some(LeaseEnforcementState::Revoked),
                    ..LeasePatch::default()
                },
            )
            .await
            .unwrap(),
        "the rejected insert must not disturb the active lease"
    );
    txn.workspaces()
        .insert_lease(new_exclusive_lease(
            second,
            workspace,
            seeded.run,
            LeaseEnforcementState::Active,
        ))
        .await
        .expect("a revoked exclusive lease releases the workspace");
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let first_row = read.workspaces().get_lease(first).await.unwrap().unwrap();
    assert_eq!(first_row.enforcement_state, LeaseEnforcementState::Revoked);
    let second_row = read.workspaces().get_lease(second).await.unwrap().unwrap();
    assert_eq!(second_row.enforcement_state, LeaseEnforcementState::Active);
}

#[tokio::test]
async fn dependency_cycles_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let second = RunId::new(&provider);
    let third = RunId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    for run in [second, third] {
        txn.runs()
            .insert(NewRun {
                run_id: run,
                task_id: seeded.task,
                session_id: None,
                parent_run_id: None,
                state: RunState::Created,
                recovery: RecoveryDisposition::Normal,
                loop_epoch: 0,
                step_sequence: 0,
                input_event_cursor: cursor(run),
                cancellation_epoch: 0,
                resolved_environment_id: None,
                created_at_ms: 10,
            })
            .await
            .unwrap();
    }
    txn.graph().ensure_head(seeded.task).await.unwrap();
    let edge = |source: RunId, target: RunId| NewRunDependency {
        dependency_id: DependencyId::new(&provider),
        task_id: seeded.task,
        source_run_id: source,
        target_run_id: target,
        dependency_condition: DependencyCondition::CompletedSuccessfully,
        created_at_ms: 20,
    };
    txn.graph()
        .insert_dependency(edge(seeded.run, second), 0)
        .await
        .unwrap();
    txn.graph()
        .insert_dependency(edge(second, third), 1)
        .await
        .unwrap();

    let back_edge = txn
        .graph()
        .insert_dependency(edge(third, seeded.run), 2)
        .await
        .unwrap_err();
    assert_eq!(back_edge.code(), ErrorCode::Conflict);
    assert_eq!(back_edge.retry_class(), RetryClass::Never);
    let self_edge = txn
        .graph()
        .insert_dependency(edge(seeded.run, seeded.run), 2)
        .await
        .unwrap_err();
    assert_eq!(self_edge.code(), ErrorCode::Conflict);
    assert_eq!(self_edge.retry_class(), RetryClass::Never);

    let head = txn.graph().get_head(seeded.task).await.unwrap().unwrap();
    assert_eq!(
        head.graph_revision, 2,
        "rejected cycles must not advance the head"
    );
    assert_eq!(
        txn.graph()
            .list_dependencies(seeded.task)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        !txn.graph().is_reachable(third, seeded.run).await.unwrap(),
        "the rejected back edge must not be stored"
    );
    drop(txn);
}

#[tokio::test]
async fn adapter_repo_crud_and_cas_instance_state() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let adapter = AdapterId::new(&provider);
    let instance = AdapterInstanceId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.adapters()
        .insert_registration(NewAdapterRegistration {
            adapter_id: adapter,
            version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            manifest_digest: "manifest-1".to_owned(),
            runtime_type: "process".to_owned(),
            implemented_ports: vec![1, 2],
            capabilities: vec![3],
            trust_state: TrustState::Trusted,
            conformance_state: ConformanceState::Untested,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    txn.adapters()
        .insert_instance(NewAdapterInstance {
            adapter_instance_id: instance,
            adapter_id: adapter,
            adapter_version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            daemon_instance_id: DaemonInstanceId::new(&provider),
            pid: Some(4242),
            process_start_identity: Some("start-1".to_owned()),
            state: "starting".to_owned(),
            exit_reason: None,
            last_heartbeat_ms: None,
            started_at_ms: 21,
            ended_at_ms: None,
        })
        .await
        .unwrap();
    let applied = txn
        .adapters()
        .cas_instance_state(
            instance,
            "starting",
            AdapterInstanceStatePatch {
                state: Some("ready".to_owned()),
                last_heartbeat_ms: Some(22),
                ..AdapterInstanceStatePatch::default()
            },
        )
        .await
        .unwrap();
    assert!(applied, "instance promotion must apply");
    let stale = txn
        .adapters()
        .cas_instance_state(
            instance,
            "starting",
            AdapterInstanceStatePatch {
                state: Some("failed".to_owned()),
                exit_reason: Some("late".to_owned()),
                ..AdapterInstanceStatePatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale, "stale instance state must not apply");
    txn.adapters()
        .insert_conformance_report(NewConformanceReport {
            adapter_id: adapter,
            adapter_version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            report_digest: "report-1".to_owned(),
            harness_version: "0.1.0".to_owned(),
            result: "pass".to_owned(),
            run_at_ms: 23,
            details: Some(vec![9]),
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let registration = read
        .adapters()
        .get_registration(adapter, "1.0.0", "bundle-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(registration.trust_state, TrustState::Trusted);
    assert_eq!(registration.runtime_type, "process");
    let report = read
        .adapters()
        .get_conformance_report(adapter, "1.0.0", "bundle-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.result, "pass");
    assert_eq!(report.details, Some(vec![9]));
    assert!(
        read.adapters()
            .get_registration(adapter, "9.9.9", "bundle-1")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn artifact_repo_crud() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let artifact = ArtifactId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.artifacts()
        .insert(NewArtifact {
            artifact_id: artifact,
            uri: "artifact://report-1".to_owned(),
            digest: "digest-1".to_owned(),
            media_type: "application/octet-stream".to_owned(),
            size_bytes: 128,
            origin_run_id: seeded.run,
            origin_effect_id: None,
            sensitivity: SensitivityClass::Internal,
            retention: RetentionClass::Standard,
            locator: "file:///var/artifacts/1".to_owned(),
            created_at_ms: 20,
        })
        .await
        .unwrap();
    let duplicate = txn
        .artifacts()
        .insert(NewArtifact {
            artifact_id: ArtifactId::new(&provider),
            uri: "artifact://report-1".to_owned(),
            digest: "digest-2".to_owned(),
            media_type: "text/plain".to_owned(),
            size_bytes: 1,
            origin_run_id: seeded.run,
            origin_effect_id: None,
            sensitivity: SensitivityClass::Public,
            retention: RetentionClass::Audit,
            locator: "file:///var/artifacts/2".to_owned(),
            created_at_ms: 21,
        })
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
    assert_eq!(duplicate.retry_class(), RetryClass::Never);
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let by_id = read.artifacts().get_by_id(artifact).await.unwrap().unwrap();
    assert_eq!(by_id.uri, "artifact://report-1");
    assert_eq!(by_id.sensitivity, SensitivityClass::Internal);
    let by_uri = read
        .artifacts()
        .get_by_uri("artifact://report-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_uri.artifact_id, artifact);
    assert!(
        read.artifacts()
            .get_by_uri("artifact://missing")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn loop_turn_repo_crud_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let turn = TurnId::new(&provider);
    let decision = DecisionId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.loop_turns()
        .insert_turn(NewLoopTurn {
            turn_id: turn,
            run_id: seeded.run,
            run_revision: 0,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(seeded.run),
            state: "issued".to_owned(),
            issued_at_ms: 20,
        })
        .await
        .unwrap();
    let applied = txn
        .loop_turns()
        .cas_turn(
            turn,
            "issued",
            LoopTurnPatch {
                state: Some("accepted".to_owned()),
            },
        )
        .await
        .unwrap();
    assert!(applied, "turn acceptance must apply");
    let stale = txn
        .loop_turns()
        .cas_turn(
            turn,
            "issued",
            LoopTurnPatch {
                state: Some("stale".to_owned()),
            },
        )
        .await
        .unwrap();
    assert!(!stale, "stale turn state must not apply");
    txn.loop_turns()
        .insert_decision(NewDecision {
            decision_id: decision,
            run_id: seeded.run,
            turn_id: turn,
            decision_type: "InvokeEffect".to_owned(),
            decision_digest: "decision-digest".to_owned(),
            decision_bytes: vec![1, 0, 1],
            run_revision: 0,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(seeded.run),
            accepted_at_ms: 21,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let turn_row = read.loop_turns().get_turn(turn).await.unwrap().unwrap();
    assert_eq!(
        turn_row.state, "accepted",
        "only the matching CAS may mutate"
    );
    assert_eq!(turn_row.issued_at_ms, 20);
    let decision_row = read
        .loop_turns()
        .get_decision(seeded.run, decision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decision_row.decision_type, "InvokeEffect");
    assert_eq!(decision_row.decision_bytes, vec![1, 0, 1]);
    assert!(
        read.loop_turns()
            .get_turn(TurnId::new(&provider))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn unknown_persisted_values_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let effect = EffectId::new(&provider);
    let timer = TimerId::new(&provider);
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.effects()
        .insert(new_effect(&provider, effect, seeded.run))
        .await
        .unwrap();
    txn.timers()
        .insert(NewTimer {
            timer_id: timer,
            run_id: Some(seeded.run),
            timer_kind: "deadline".to_owned(),
            payload: vec![4, 5],
            due_at_ms: 100,
            state: TimerState::Scheduled,
            version: 0,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    drop(store);

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE effects SET state = 99 WHERE effect_id = ?")
        .bind(effect.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE timers SET state = 'bogus' WHERE timer_id = ?")
        .bind(timer.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    drop(pool);

    let store = SqliteKernelStore::open(config(&db_path)).await.unwrap();
    let mut read = store.begin_read().await.unwrap();
    let effect_error = read.effects().get(effect).await.unwrap_err();
    assert_eq!(effect_error.code(), ErrorCode::Internal);
    assert_eq!(effect_error.retry_class(), RetryClass::Never);
    let timer_error = read.timers().get(timer).await.unwrap_err();
    assert_eq!(timer_error.code(), ErrorCode::Internal);
    assert_eq!(timer_error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn unknown_text_literals_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    let turn = TurnId::new(&provider);
    let decision = DecisionId::new(&provider);
    let adapter = AdapterId::new(&provider);
    let instance = AdapterInstanceId::new(&provider);
    let valid = ConfigGenerationId::new(&provider);
    let bad_test_state = ConfigGenerationId::new(&provider);
    let request = ApprovalRequestId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.loop_turns()
        .insert_turn(NewLoopTurn {
            turn_id: turn,
            run_id: seeded.run,
            run_revision: 0,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(seeded.run),
            state: "issued".to_owned(),
            issued_at_ms: 20,
        })
        .await
        .unwrap();
    txn.loop_turns()
        .insert_decision(NewDecision {
            decision_id: decision,
            run_id: seeded.run,
            turn_id: turn,
            decision_type: "InvokeEffect".to_owned(),
            decision_digest: "decision-digest".to_owned(),
            decision_bytes: vec![1],
            run_revision: 0,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(seeded.run),
            accepted_at_ms: 21,
        })
        .await
        .unwrap();
    txn.adapters()
        .insert_registration(NewAdapterRegistration {
            adapter_id: adapter,
            version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            manifest_digest: "manifest-1".to_owned(),
            runtime_type: "process".to_owned(),
            implemented_ports: vec![],
            capabilities: vec![],
            trust_state: TrustState::Trusted,
            conformance_state: ConformanceState::Untested,
            created_at_ms: 20,
        })
        .await
        .unwrap();
    txn.adapters()
        .insert_instance(NewAdapterInstance {
            adapter_instance_id: instance,
            adapter_id: adapter,
            adapter_version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            daemon_instance_id: DaemonInstanceId::new(&provider),
            pid: None,
            process_start_identity: None,
            state: "starting".to_owned(),
            exit_reason: None,
            last_heartbeat_ms: None,
            started_at_ms: 21,
            ended_at_ms: None,
        })
        .await
        .unwrap();
    txn.config()
        .insert_generation(NewConfigGeneration {
            generation_id: valid,
            digest: "digest-valid".to_owned(),
            document: vec![7],
            validation_state: "validated".to_owned(),
            test_state: "passed".to_owned(),
            created_by_actor_id: ActorId::new(&provider),
            created_at_ms: 22,
        })
        .await
        .unwrap();
    txn.config()
        .insert_generation(NewConfigGeneration {
            generation_id: bad_test_state,
            digest: "digest-bad-test-state".to_owned(),
            document: vec![8],
            validation_state: "proposed".to_owned(),
            test_state: "untested".to_owned(),
            created_by_actor_id: ActorId::new(&provider),
            created_at_ms: 23,
        })
        .await
        .unwrap();
    txn.adapters()
        .insert_conformance_report(NewConformanceReport {
            adapter_id: adapter,
            adapter_version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            report_digest: "report-1".to_owned(),
            harness_version: "0.1.0".to_owned(),
            result: "pass".to_owned(),
            run_at_ms: 24,
            details: None,
        })
        .await
        .unwrap();
    txn.security()
        .insert_approval_request(NewApprovalRequest {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            principal_id: PrincipalId::new(&provider),
            actor_id: ActorId::new(&provider),
            run_id: Some(seeded.run),
            operation: "write-file".to_owned(),
            target_resource: None,
            capability_ids: vec![1],
            extension_bundle_digest: None,
            config_generation_digest: None,
            expires_at_ms: 1_900_000_000_000,
            nonce: "nonce-1".to_owned(),
            state: ApprovalState::Pending,
            created_at_ms: 20,
            resolved_at_ms: None,
        })
        .await
        .unwrap();
    txn.security()
        .insert_approval_response(NewApprovalResponse {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            decision: "approve".to_owned(),
            device_id: domain::ids::DeviceId::new(&provider),
            responder_principal_id: PrincipalId::new(&provider),
            responded_at_ms: 30,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    drop(store);

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    for statement in [
        "UPDATE loop_turns SET state = 'bogus'",
        "UPDATE decisions SET decision_type = 'Bogus'",
        "UPDATE adapter_instances SET state = 'bogus'",
        "UPDATE config_generations SET validation_state = 'bogus' WHERE digest = 'digest-valid'",
        "UPDATE config_generations SET test_state = 'bogus' \
         WHERE digest = 'digest-bad-test-state'",
        "DROP TRIGGER trg_conformance_reports_update",
        "UPDATE conformance_reports SET result = 'bogus'",
        "UPDATE approval_responses SET decision = 'bogus'",
    ] {
        sqlx::query(statement).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    drop(pool);

    let store = SqliteKernelStore::open(config(&db_path)).await.unwrap();
    let mut read = store.begin_read().await.unwrap();
    let turn_error = read.loop_turns().get_turn(turn).await.unwrap_err();
    assert_eq!(turn_error.code(), ErrorCode::Internal);
    assert_eq!(turn_error.retry_class(), RetryClass::Never);
    let decision_error = read
        .loop_turns()
        .get_decision(seeded.run, decision)
        .await
        .unwrap_err();
    assert_eq!(decision_error.code(), ErrorCode::Internal);
    assert_eq!(decision_error.retry_class(), RetryClass::Never);
    let validation_error = read.config().get_generation(valid).await.unwrap_err();
    assert_eq!(validation_error.code(), ErrorCode::Internal);
    assert_eq!(validation_error.retry_class(), RetryClass::Never);
    let test_state_error = read
        .config()
        .get_generation(bad_test_state)
        .await
        .unwrap_err();
    assert_eq!(test_state_error.code(), ErrorCode::Internal);
    assert_eq!(test_state_error.retry_class(), RetryClass::Never);
    let report_error = read
        .adapters()
        .get_conformance_report(adapter, "1.0.0", "bundle-1")
        .await
        .unwrap_err();
    assert_eq!(report_error.code(), ErrorCode::Internal);
    assert_eq!(report_error.retry_class(), RetryClass::Never);
    let response_error = read
        .security()
        .list_approval_responses(request)
        .await
        .unwrap_err();
    assert_eq!(response_error.code(), ErrorCode::Internal);
    assert_eq!(response_error.retry_class(), RetryClass::Never);
    drop(read);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let instance_error = txn
        .adapters()
        .cas_instance_state(
            instance,
            "starting",
            AdapterInstanceStatePatch {
                state: Some("ready".to_owned()),
                ..AdapterInstanceStatePatch::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(instance_error.code(), ErrorCode::Internal);
    assert_eq!(instance_error.retry_class(), RetryClass::Never);
}
