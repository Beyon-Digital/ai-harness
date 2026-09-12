//! Contract test for the in-memory `KernelStore` mock.
//!
//! The mock is the second implementation of the port. These tests prove the
//! contract is implementable: atomic visibility on commit, no residue on drop,
//! CAS conditions that reject stale expectations, idempotency replay,
//! contiguous stream allocation, and the daemon-epoch assertion at write begin.

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{
    ActorId, AdapterId, AdapterInstanceId, ApprovalRequestId, ArtifactId, CapabilityGrantId,
    CommandId, ConfigGenerationId, DaemonInstanceId, DecisionId, DelegationChainId, DependencyId,
    EffectId, EventCursor, EventId, EventStreamKey, IdempotencyKey, LeaseId, PrincipalId, RunId,
    SessionId, TaskId, TimerId, TurnId, WorkspaceId,
};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{
    ApprovalState, ConformanceState, RetentionClass, SensitivityClass, TrustState,
};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::{
    AdapterInstanceStatePatch, EffectPatch, KernelStore, LeasePatch, LoopTurnPatch,
    NewAdapterInstance, NewAdapterRegistration, NewApprovalRequest, NewApprovalResponse,
    NewArtifact, NewCapabilityGrant, NewConfigGeneration, NewConformanceReport, NewDecision,
    NewDelegationHop, NewEffect, NewIdempotencyRecord, NewLoopTurn, NewOutboxEvent, NewRun,
    NewRunDependency, NewSession, NewTask, NewTimer, NewWorkspace, NewWorkspaceLease, RunCas,
    RunPatch, TxContext,
};
use testkit::ids::DeterministicIds;
use testkit::store::MockStore;

const SEED_MS: i64 = 1_700_000_000_000;

fn context(ids: &DeterministicIds) -> TxContext {
    TxContext {
        daemon_epoch: 1,
        principal_id: PrincipalId::new(ids),
        command_id: CommandId::new(ids),
        correlation_id: Some("corr-1".to_owned()),
    }
}

fn context_with_epoch(ids: &DeterministicIds, epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch: epoch,
        principal_id: PrincipalId::new(ids),
        command_id: CommandId::new(ids),
        correlation_id: None,
    }
}

fn stream_key(run: RunId) -> EventStreamKey {
    match EventStreamKey::new(format!("run/{run}")) {
        Ok(key) => key,
        Err(error) => panic!("stream key rejected: {error}"),
    }
}

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(stream_key(run), 0)
}

struct Fixture {
    ids: DeterministicIds,
    store: MockStore,
    session_id: SessionId,
    task_id: TaskId,
    run_id: RunId,
    effect_id: EffectId,
    event_id: EventId,
    principal_id: PrincipalId,
}

fn fixture() -> Fixture {
    let ids = DeterministicIds::new(SEED_MS);
    let store = MockStore::new();
    store
        .set_daemon_epoch(1, DaemonInstanceId::new(&ids))
        .expect("seed the persisted daemon epoch");
    Fixture {
        session_id: SessionId::new(&ids),
        task_id: TaskId::new(&ids),
        run_id: RunId::new(&ids),
        effect_id: EffectId::new(&ids),
        event_id: EventId::new(&ids),
        principal_id: PrincipalId::new(&ids),
        store,
        ids,
    }
}

fn new_session(f: &Fixture) -> NewSession {
    NewSession {
        session_id: f.session_id,
        principal_id: f.principal_id,
        created_at_ms: SEED_MS,
        metadata: None,
    }
}

fn new_task(f: &Fixture) -> NewTask {
    NewTask {
        task_id: f.task_id,
        session_id: Some(f.session_id),
        created_by_actor_id: ActorId::new(&f.ids),
        task_kind: "agent_run".to_owned(),
        payload: vec![0x01, 0x02],
        created_at_ms: SEED_MS,
    }
}

fn new_run(f: &Fixture) -> NewRun {
    NewRun {
        run_id: f.run_id,
        task_id: f.task_id,
        session_id: Some(f.session_id),
        parent_run_id: None,
        state: RunState::Created,
        recovery: RecoveryDisposition::Normal,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: cursor(f.run_id),
        cancellation_epoch: 0,
        resolved_environment_id: None,
        created_at_ms: SEED_MS,
    }
}

fn new_effect(f: &Fixture) -> NewEffect {
    NewEffect {
        effect_id: f.effect_id,
        run_id: f.run_id,
        step_sequence: 0,
        decision_id: DecisionId::new(&f.ids),
        operation: "fs.write".to_owned(),
        request_hash: "hash-1".to_owned(),
        request_payload: vec![0x00],
        effect_class: EffectClass::LocalMutation,
        idempotency_semantics: IdempotencySemantics::IdempotencyKeySupported,
        reconciliation_semantics: ReconciliationSemantics::StatusLookup,
        cancellation_semantics: "abandon".to_owned(),
        compensation_capability: None,
        adapter_id: AdapterId::new(&f.ids),
        adapter_version: "1.0.0".to_owned(),
        adapter_digest: "digest-1".to_owned(),
        state: EffectState::Prepared,
        created_at_ms: SEED_MS,
    }
}

fn new_outbox(f: &Fixture, sequence: u64) -> NewOutboxEvent {
    NewOutboxEvent {
        event_id: f.event_id,
        event_type: "run.created".to_owned(),
        event_version: 1,
        stream_key: stream_key(f.run_id),
        sequence,
        occurred_at_ms: SEED_MS,
        run_id: Some(f.run_id),
        task_id: Some(f.task_id),
        session_id: Some(f.session_id),
        effect_id: None,
        causation_id: None,
        correlation_id: Some("corr-1".to_owned()),
        sensitivity: SensitivityClass::Internal,
        retention: RetentionClass::Audit,
        payload: vec![0x07],
    }
}

fn new_idempotency(f: &Fixture, key: &IdempotencyKey, digest: &str) -> NewIdempotencyRecord {
    NewIdempotencyRecord {
        principal_id: f.principal_id,
        idempotency_key: key.clone(),
        request_digest: digest.to_owned(),
        command_id: CommandId::new(&f.ids),
        outcome_code: "ok".to_owned(),
        outcome_payload: vec![0x00],
        created_at_ms: SEED_MS,
    }
}

#[tokio::test]
async fn committed_mutations_are_visible_atomically() {
    let f = fixture();
    let key = match IdempotencyKey::new("command-1") {
        Ok(key) => key,
        Err(error) => panic!("idempotency key rejected: {error}"),
    };

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.sessions()
        .insert(new_session(&f))
        .await
        .expect("session");
    txn.tasks().insert(new_task(&f)).await.expect("task");
    txn.runs().insert(new_run(&f)).await.expect("run");
    let sequence = txn
        .streams()
        .allocate(stream_key(f.run_id))
        .await
        .expect("allocate");
    assert_eq!(sequence, 1);
    txn.streams()
        .insert_outbox(new_outbox(&f, sequence))
        .await
        .expect("outbox");
    txn.effects().insert(new_effect(&f)).await.expect("effect");
    assert!(
        txn.idempotency()
            .lookup(f.principal_id, &key)
            .await
            .expect("lookup")
            .is_none()
    );
    txn.idempotency()
        .insert(new_idempotency(&f, &key, "digest-1"))
        .await
        .expect("idempotency");

    {
        let mut read = f.store.begin_read().await.expect("begin read");
        assert!(read.runs().get(f.run_id).await.expect("run").is_none());
        assert!(read.tasks().get(f.task_id).await.expect("task").is_none());
        assert!(
            read.sessions()
                .get(f.session_id)
                .await
                .expect("session")
                .is_none()
        );
        assert!(
            read.effects()
                .get(f.effect_id)
                .await
                .expect("effect")
                .is_none()
        );
    }

    txn.commit().await.expect("commit");

    let mut read = f.store.begin_read().await.expect("begin read");
    let run = read
        .runs()
        .get(f.run_id)
        .await
        .expect("run")
        .expect("run row");
    assert_eq!(run.state, RunState::Created);
    assert_eq!(run.run_revision, 0);
    assert_eq!(run.input_event_cursor, cursor(f.run_id));
    assert!(read.tasks().get(f.task_id).await.expect("task").is_some());
    assert!(
        read.sessions()
            .get(f.session_id)
            .await
            .expect("session")
            .is_some()
    );
    assert!(
        read.effects()
            .get(f.effect_id)
            .await
            .expect("effect")
            .is_some()
    );

    let mut verify = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin verify");
    let unpublished = verify
        .streams()
        .scan_unpublished(10)
        .await
        .expect("scan outbox");
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].event_id, f.event_id);
    assert_eq!(unpublished[0].sequence, 1);
    assert!(unpublished[0].journal_published_at_ms.is_none());
    let replay = verify
        .idempotency()
        .lookup(f.principal_id, &key)
        .await
        .expect("lookup")
        .expect("stored record");
    assert_eq!(replay.outcome_code, "ok");
    assert_eq!(replay.request_digest, "digest-1");
    verify.rollback().await.expect("rollback");
}

#[tokio::test]
async fn dropped_transaction_leaves_no_state() {
    let f = fixture();
    {
        let mut txn = f
            .store
            .begin_write(context(&f.ids))
            .await
            .expect("begin write");
        txn.sessions()
            .insert(new_session(&f))
            .await
            .expect("session");
        txn.tasks().insert(new_task(&f)).await.expect("task");
        txn.runs().insert(new_run(&f)).await.expect("run");
        drop(txn);
    }

    let mut read = f.store.begin_read().await.expect("begin read");
    assert!(read.runs().get(f.run_id).await.expect("run").is_none());
    assert!(read.tasks().get(f.task_id).await.expect("task").is_none());
    assert!(
        read.sessions()
            .get(f.session_id)
            .await
            .expect("session")
            .is_none()
    );
}

#[tokio::test]
async fn cas_conditions_reject_stale_expectations() {
    let f = fixture();
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.sessions()
        .insert(new_session(&f))
        .await
        .expect("session");
    txn.tasks().insert(new_task(&f)).await.expect("task");
    txn.runs().insert(new_run(&f)).await.expect("run");
    txn.effects().insert(new_effect(&f)).await.expect("effect");
    txn.commit().await.expect("commit");

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let stale = RunCas {
        run_revision: 9,
        state: None,
        cancellation_epoch: None,
    };
    assert!(
        !txn.runs()
            .cas_update(
                f.run_id,
                stale,
                RunPatch {
                    state: Some(RunState::Ready),
                    bump_revision: true,
                    ..RunPatch::default()
                }
            )
            .await
            .expect("cas")
    );
    let current = RunCas {
        run_revision: 0,
        state: Some(RunState::Created),
        cancellation_epoch: Some(0),
    };
    assert!(
        txn.runs()
            .cas_update(
                f.run_id,
                current,
                RunPatch {
                    state: Some(RunState::Ready),
                    bump_revision: true,
                    ..RunPatch::default()
                }
            )
            .await
            .expect("cas")
    );

    assert!(
        !txn.effects()
            .cas_transition(
                f.effect_id,
                EffectState::Dispatched,
                None,
                EffectPatch::default()
            )
            .await
            .expect("cas")
    );
    assert!(
        txn.effects()
            .cas_transition(
                f.effect_id,
                EffectState::Prepared,
                None,
                EffectPatch {
                    state: Some(EffectState::Claimed),
                    executor_id: Some("exec-1".to_owned()),
                    executor_fencing_token: Some(7),
                    ..EffectPatch::default()
                }
            )
            .await
            .expect("cas")
    );
    assert!(
        !txn.effects()
            .cas_transition(
                f.effect_id,
                EffectState::Claimed,
                Some(8),
                EffectPatch {
                    state: Some(EffectState::Dispatched),
                    ..EffectPatch::default()
                }
            )
            .await
            .expect("cas")
    );
    assert!(
        txn.effects()
            .cas_transition(
                f.effect_id,
                EffectState::Claimed,
                Some(7),
                EffectPatch {
                    state: Some(EffectState::Dispatched),
                    ..EffectPatch::default()
                }
            )
            .await
            .expect("cas")
    );
    txn.commit().await.expect("commit");

    let mut read = f.store.begin_read().await.expect("begin read");
    let run = read
        .runs()
        .get(f.run_id)
        .await
        .expect("run")
        .expect("run row");
    assert_eq!(run.state, RunState::Ready);
    assert_eq!(run.run_revision, 1);
    let effect = read
        .effects()
        .get(f.effect_id)
        .await
        .expect("effect")
        .expect("effect row");
    assert_eq!(effect.state, EffectState::Dispatched);
    assert_eq!(effect.executor_fencing_token, Some(7));
}

#[tokio::test]
async fn idempotency_replay_and_duplicate_keys_conflict() {
    let f = fixture();
    let key = match IdempotencyKey::new("command-1") {
        Ok(key) => key,
        Err(error) => panic!("idempotency key rejected: {error}"),
    };
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.idempotency()
        .insert(new_idempotency(&f, &key, "digest-1"))
        .await
        .expect("insert");
    txn.commit().await.expect("commit");

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let replay = txn
        .idempotency()
        .lookup(f.principal_id, &key)
        .await
        .expect("lookup")
        .expect("stored record");
    assert_eq!(replay.outcome_payload, vec![0x00]);
    let error = txn
        .idempotency()
        .insert(new_idempotency(&f, &key, "different-digest"))
        .await
        .expect_err("duplicate key must conflict");
    assert_eq!(error.code(), ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn stream_allocation_is_contiguous_across_transactions() {
    let f = fixture();
    let run_stream = stream_key(f.run_id);
    let config_stream = match EventStreamKey::new("config/global") {
        Ok(key) => key,
        Err(error) => panic!("stream key rejected: {error}"),
    };

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    assert_eq!(
        txn.streams().allocate(run_stream.clone()).await.expect("a"),
        1
    );
    assert_eq!(
        txn.streams().allocate(run_stream.clone()).await.expect("b"),
        2
    );
    assert_eq!(
        txn.streams()
            .allocate(config_stream.clone())
            .await
            .expect("c"),
        1
    );
    txn.commit().await.expect("commit");

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    assert_eq!(txn.streams().allocate(run_stream).await.expect("d"), 3);
    assert_eq!(txn.streams().allocate(config_stream).await.expect("e"), 2);
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn begin_write_asserts_the_persisted_daemon_epoch() {
    let ids = DeterministicIds::new(SEED_MS);
    let store = MockStore::new();

    let error = match store.begin_write(context_with_epoch(&ids, 1)).await {
        Ok(_) => panic!("epoch 1 must be refused while no fence is recorded"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    let error = match store.begin_write(context_with_epoch(&ids, 0)).await {
        Ok(_) => panic!("epoch 0 is the absent epoch and must be refused"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);

    store
        .set_daemon_epoch(2, DaemonInstanceId::new(&ids))
        .expect("simulate a persisted fence epoch");
    let error = match store.begin_write(context_with_epoch(&ids, 1)).await {
        Ok(_) => panic!("stale epoch 1 must be refused"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);

    let txn = store
        .begin_write(context_with_epoch(&ids, 2))
        .await
        .expect("matching epoch must open a write transaction");
    txn.rollback().await.expect("rollback");

    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("acquire fence");
    assert_eq!(fence.epoch.0, 3);
    let txn = store
        .begin_write(context_with_epoch(&ids, 3))
        .await
        .expect("epoch recorded by acquire must open a write transaction");
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn fence_acquisition_invalidates_in_flight_write() {
    let ids = DeterministicIds::new(SEED_MS);
    let store = MockStore::new();
    let instance = DaemonInstanceId::new(&ids);
    store
        .set_daemon_epoch(1, instance)
        .expect("seed the persisted fence at epoch 1");

    let txn = store
        .begin_write(context_with_epoch(&ids, 1))
        .await
        .expect("open a write transaction at epoch 1");
    let fence = store
        .acquire_daemon_fence(instance)
        .await
        .expect("acquire fence");
    assert_eq!(fence.epoch.0, 2);

    let error = match txn.commit().await {
        Ok(()) => panic!("in-flight transaction must fail after fence acquisition"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::Conflict);

    let error = match store.begin_write(context_with_epoch(&ids, 1)).await {
        Ok(_) => panic!("stale epoch 1 must be refused after the fence moved to 2"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);

    let txn = store
        .begin_write(context_with_epoch(&ids, 2))
        .await
        .expect("epoch 2 must open a write transaction");
    txn.rollback().await.expect("rollback");

    let current = store
        .current_fence()
        .await
        .expect("current fence")
        .expect("the new fence must survive the failed commit");
    assert_eq!(current.epoch.0, 2);
    assert_eq!(current.instance_id, instance);
}

fn assert_error(error: KernelError, code: ErrorCode) {
    assert_eq!(error.code(), code);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

async fn seed_base(f: &Fixture) {
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.sessions()
        .insert(new_session(f))
        .await
        .expect("session");
    txn.tasks().insert(new_task(f)).await.expect("task");
    txn.runs().insert(new_run(f)).await.expect("run");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn graph_dependency_cycles_are_rejected() {
    let f = fixture();
    seed_base(&f).await;
    let second = RunId::new(&f.ids);
    let third = RunId::new(&f.ids);
    let dependency = |source: RunId, target: RunId| NewRunDependency {
        dependency_id: DependencyId::new(&f.ids),
        task_id: f.task_id,
        source_run_id: source,
        target_run_id: target,
        dependency_condition: DependencyCondition::CompletedSuccessfully,
        created_at_ms: SEED_MS,
    };

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    for run in [second, third] {
        txn.runs()
            .insert(NewRun {
                run_id: run,
                ..new_run(&f)
            })
            .await
            .expect("extra run");
    }
    txn.graph().ensure_head(f.task_id).await.expect("head");
    txn.graph()
        .insert_dependency(dependency(f.run_id, second), 0)
        .await
        .expect("first edge");
    txn.graph()
        .insert_dependency(dependency(second, third), 1)
        .await
        .expect("second edge");

    let back_edge = txn
        .graph()
        .insert_dependency(dependency(third, f.run_id), 2)
        .await
        .expect_err("a back edge must be rejected");
    assert_error(back_edge, ErrorCode::Conflict);
    let self_edge = txn
        .graph()
        .insert_dependency(dependency(f.run_id, f.run_id), 2)
        .await
        .expect_err("a self edge must be rejected");
    assert_error(self_edge, ErrorCode::Conflict);

    let head = txn
        .graph()
        .get_head(f.task_id)
        .await
        .expect("head")
        .expect("head row");
    assert_eq!(head.graph_revision, 2, "rejected cycles must not advance");
    assert_eq!(
        txn.graph()
            .list_dependencies(f.task_id)
            .await
            .expect("list")
            .len(),
        2
    );
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn active_exclusive_workspace_leases_conflict() {
    let f = fixture();
    seed_base(&f).await;
    let workspace = WorkspaceId::new(&f.ids);
    let first = LeaseId::new(&f.ids);
    let second = LeaseId::new(&f.ids);
    let lease = |id: LeaseId, enforcement_state: LeaseEnforcementState| NewWorkspaceLease {
        lease_id: id,
        workspace_id: workspace,
        owner_run_id: f.run_id,
        mode: WorkspaceAccessMode::ExclusiveWrite,
        lease_epoch: 1,
        enforcement_state,
        delegated_from: vec![],
        created_at_ms: SEED_MS,
    };

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.workspaces()
        .insert_workspace(NewWorkspace {
            workspace_id: workspace,
            kind: "git".to_owned(),
            base_revision: None,
            parent_workspace_id: None,
            created_at_ms: SEED_MS,
        })
        .await
        .expect("workspace");
    txn.workspaces()
        .insert_lease(lease(first, LeaseEnforcementState::Active))
        .await
        .expect("first lease");
    let duplicate = txn
        .workspaces()
        .insert_lease(lease(second, LeaseEnforcementState::Active))
        .await
        .expect_err("a second active exclusive lease must conflict");
    assert_error(duplicate, ErrorCode::Conflict);
    txn.workspaces()
        .insert_lease(lease(second, LeaseEnforcementState::Revoked))
        .await
        .expect("a revoked exclusive lease may coexist");
    txn.commit().await.expect("commit");

    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let activate = txn
        .workspaces()
        .cas_lease(
            second,
            1,
            LeasePatch {
                enforcement_state: Some(LeaseEnforcementState::Active),
                ..LeasePatch::default()
            },
        )
        .await
        .expect_err("activating a second exclusive lease must conflict");
    assert_error(activate, ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn loop_turn_and_decision_literals_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let turn = TurnId::new(&f.ids);
    let turn_fields = |state: &str| NewLoopTurn {
        turn_id: turn,
        run_id: f.run_id,
        run_revision: 0,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: cursor(f.run_id),
        state: state.to_owned(),
        issued_at_ms: SEED_MS,
    };
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");

    let bad_state = txn
        .loop_turns()
        .insert_turn(turn_fields("bogus"))
        .await
        .expect_err("an unknown turn state must fail");
    assert_error(bad_state, ErrorCode::FailedPrecondition);
    let missing_run = txn
        .loop_turns()
        .insert_turn(NewLoopTurn {
            run_id: RunId::new(&f.ids),
            ..turn_fields("issued")
        })
        .await
        .expect_err("a turn must reference an existing run");
    assert_error(missing_run, ErrorCode::FailedPrecondition);
    txn.loop_turns()
        .insert_turn(turn_fields("issued"))
        .await
        .expect("turn");
    let bad_patch = txn
        .loop_turns()
        .cas_turn(
            turn,
            "issued",
            LoopTurnPatch {
                state: Some("bogus".to_owned()),
            },
        )
        .await
        .expect_err("an unknown turn patch state must fail");
    assert_error(bad_patch, ErrorCode::FailedPrecondition);

    let decision = |decision_type: &str, turn_id: TurnId| NewDecision {
        decision_id: DecisionId::new(&f.ids),
        run_id: f.run_id,
        turn_id,
        decision_type: decision_type.to_owned(),
        decision_digest: "decision-digest".to_owned(),
        decision_bytes: vec![1],
        run_revision: 0,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: cursor(f.run_id),
        accepted_at_ms: SEED_MS,
    };
    let bad_type = txn
        .loop_turns()
        .insert_decision(decision("Bogus", turn))
        .await
        .expect_err("an unknown decision type must fail");
    assert_error(bad_type, ErrorCode::FailedPrecondition);
    let missing_turn = txn
        .loop_turns()
        .insert_decision(decision("InvokeEffect", TurnId::new(&f.ids)))
        .await
        .expect_err("a decision must reference an existing turn");
    assert_error(missing_turn, ErrorCode::FailedPrecondition);
    let missing_run = txn
        .loop_turns()
        .insert_decision(NewDecision {
            run_id: RunId::new(&f.ids),
            ..decision("InvokeEffect", turn)
        })
        .await
        .expect_err("a decision must reference an existing run");
    assert_error(missing_run, ErrorCode::FailedPrecondition);
    txn.loop_turns()
        .insert_decision(decision("InvokeEffect", turn))
        .await
        .expect("decision");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn adapter_instance_and_conformance_literals_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let adapter = AdapterId::new(&f.ids);
    let instance_id = AdapterInstanceId::new(&f.ids);
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
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
            created_at_ms: SEED_MS,
        })
        .await
        .expect("registration");

    let instance = |state: &str| NewAdapterInstance {
        adapter_instance_id: instance_id,
        adapter_id: adapter,
        adapter_version: "1.0.0".to_owned(),
        bundle_digest: "bundle-1".to_owned(),
        daemon_instance_id: DaemonInstanceId::new(&f.ids),
        pid: None,
        process_start_identity: None,
        state: state.to_owned(),
        exit_reason: None,
        last_heartbeat_ms: None,
        started_at_ms: SEED_MS,
        ended_at_ms: None,
    };
    let bad_state = txn
        .adapters()
        .insert_instance(instance("bogus"))
        .await
        .expect_err("an unknown instance state must fail");
    assert_error(bad_state, ErrorCode::FailedPrecondition);
    let missing_registration = txn
        .adapters()
        .insert_instance(NewAdapterInstance {
            adapter_id: AdapterId::new(&f.ids),
            ..instance("starting")
        })
        .await
        .expect_err("an instance must reference an existing registration");
    assert_error(missing_registration, ErrorCode::FailedPrecondition);
    txn.adapters()
        .insert_instance(instance("starting"))
        .await
        .expect("instance");
    let bad_patch = txn
        .adapters()
        .cas_instance_state(
            instance_id,
            "starting",
            AdapterInstanceStatePatch {
                state: Some("bogus".to_owned()),
                ..AdapterInstanceStatePatch::default()
            },
        )
        .await
        .expect_err("an unknown instance patch state must fail");
    assert_error(bad_patch, ErrorCode::FailedPrecondition);

    let bad_result = txn
        .adapters()
        .insert_conformance_report(NewConformanceReport {
            adapter_id: adapter,
            adapter_version: "1.0.0".to_owned(),
            bundle_digest: "bundle-1".to_owned(),
            report_digest: "report-1".to_owned(),
            harness_version: "0.1.0".to_owned(),
            result: "bogus".to_owned(),
            run_at_ms: SEED_MS,
            details: None,
        })
        .await
        .expect_err("an unknown conformance result must fail");
    assert_error(bad_result, ErrorCode::FailedPrecondition);
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn config_generation_literals_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let generation = |validation_state: &str, test_state: &str| NewConfigGeneration {
        generation_id: ConfigGenerationId::new(&f.ids),
        digest: format!("digest-{validation_state}-{test_state}"),
        document: vec![7],
        validation_state: validation_state.to_owned(),
        test_state: test_state.to_owned(),
        created_by_actor_id: ActorId::new(&f.ids),
        created_at_ms: SEED_MS,
    };
    let bad_validation = txn
        .config()
        .insert_generation(generation("bogus", "untested"))
        .await
        .expect_err("an unknown validation state must fail");
    assert_error(bad_validation, ErrorCode::FailedPrecondition);
    let bad_test_state = txn
        .config()
        .insert_generation(generation("proposed", "bogus"))
        .await
        .expect_err("an unknown test state must fail");
    assert_error(bad_test_state, ErrorCode::FailedPrecondition);
    txn.config()
        .insert_generation(generation("proposed", "untested"))
        .await
        .expect("generation");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn approval_response_decision_literal_is_validated() {
    let f = fixture();
    seed_base(&f).await;
    let request = ApprovalRequestId::new(&f.ids);
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    txn.security()
        .insert_approval_request(NewApprovalRequest {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            principal_id: f.principal_id,
            actor_id: ActorId::new(&f.ids),
            run_id: Some(f.run_id),
            operation: "write-file".to_owned(),
            target_resource: None,
            capability_ids: vec![1],
            extension_bundle_digest: None,
            config_generation_digest: None,
            expires_at_ms: SEED_MS,
            nonce: "nonce-1".to_owned(),
            state: ApprovalState::Pending,
            created_at_ms: SEED_MS,
            resolved_at_ms: None,
        })
        .await
        .expect("approval request");
    let bad_decision = txn
        .security()
        .insert_approval_response(NewApprovalResponse {
            request_id: request,
            request_digest: "request-digest".to_owned(),
            decision: "maybe".to_owned(),
            device_id: domain::ids::DeviceId::new(&f.ids),
            responder_principal_id: f.principal_id,
            responded_at_ms: SEED_MS,
        })
        .await
        .expect_err("an unknown approval decision must fail");
    assert_error(bad_decision, ErrorCode::FailedPrecondition);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn artifact_references_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let artifact = |origin_run_id: RunId, origin_effect_id: Option<EffectId>| NewArtifact {
        artifact_id: ArtifactId::new(&f.ids),
        uri: format!("artifact://{}", ArtifactId::new(&f.ids)),
        digest: "digest-1".to_owned(),
        media_type: "application/octet-stream".to_owned(),
        size_bytes: 1,
        origin_run_id,
        origin_effect_id,
        sensitivity: SensitivityClass::Internal,
        retention: RetentionClass::Audit,
        locator: "file:///var/artifacts/1".to_owned(),
        created_at_ms: SEED_MS,
    };
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let missing_run = txn
        .artifacts()
        .insert(artifact(RunId::new(&f.ids), None))
        .await
        .expect_err("an artifact must reference an existing run");
    assert_error(missing_run, ErrorCode::FailedPrecondition);
    let missing_effect = txn
        .artifacts()
        .insert(artifact(f.run_id, Some(EffectId::new(&f.ids))))
        .await
        .expect_err("an artifact must reference an existing effect");
    assert_error(missing_effect, ErrorCode::FailedPrecondition);
    txn.effects().insert(new_effect(&f)).await.expect("effect");
    txn.artifacts()
        .insert(artifact(f.run_id, Some(f.effect_id)))
        .await
        .expect("artifact");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn timer_and_lease_references_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let workspace = WorkspaceId::new(&f.ids);
    let lease = |owner_run_id: RunId| NewWorkspaceLease {
        lease_id: LeaseId::new(&f.ids),
        workspace_id: workspace,
        owner_run_id,
        mode: WorkspaceAccessMode::ExclusiveWrite,
        lease_epoch: 1,
        enforcement_state: LeaseEnforcementState::Active,
        delegated_from: vec![],
        created_at_ms: SEED_MS,
    };
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let missing_run = txn
        .timers()
        .insert(NewTimer {
            timer_id: TimerId::new(&f.ids),
            run_id: Some(RunId::new(&f.ids)),
            timer_kind: "deadline".to_owned(),
            payload: vec![4, 5],
            due_at_ms: 100,
            state: TimerState::Scheduled,
            version: 0,
            created_at_ms: SEED_MS,
        })
        .await
        .expect_err("a timer must reference an existing run");
    assert_error(missing_run, ErrorCode::FailedPrecondition);
    let missing_workspace = txn
        .workspaces()
        .insert_lease(lease(f.run_id))
        .await
        .expect_err("a lease must reference an existing workspace");
    assert_error(missing_workspace, ErrorCode::FailedPrecondition);
    txn.workspaces()
        .insert_workspace(NewWorkspace {
            workspace_id: workspace,
            kind: "git".to_owned(),
            base_revision: None,
            parent_workspace_id: None,
            created_at_ms: SEED_MS,
        })
        .await
        .expect("workspace");
    let missing_owner = txn
        .workspaces()
        .insert_lease(lease(RunId::new(&f.ids)))
        .await
        .expect_err("a lease must reference an existing owner run");
    assert_error(missing_owner, ErrorCode::FailedPrecondition);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn grant_and_hop_references_are_validated() {
    let f = fixture();
    seed_base(&f).await;
    let mut txn = f
        .store
        .begin_write(context(&f.ids))
        .await
        .expect("begin write");
    let grant = |run_id: Option<RunId>, parent: Option<CapabilityGrantId>| NewCapabilityGrant {
        grant_id: CapabilityGrantId::new(&f.ids),
        principal_id: f.principal_id,
        actor_id: ActorId::new(&f.ids),
        run_id,
        capability_id: "filesystem.write".to_owned(),
        scope: vec![1, 2],
        delegated_from_grant_id: parent,
        expires_at_ms: None,
        revoked_at_ms: None,
        created_at_ms: SEED_MS,
    };
    let missing_run = txn
        .security()
        .insert_grant(grant(Some(RunId::new(&f.ids)), None))
        .await
        .expect_err("a grant must reference an existing run");
    assert_error(missing_run, ErrorCode::FailedPrecondition);
    let missing_parent = txn
        .security()
        .insert_grant(grant(Some(f.run_id), Some(CapabilityGrantId::new(&f.ids))))
        .await
        .expect_err("a grant must reference an existing parent grant");
    assert_error(missing_parent, ErrorCode::FailedPrecondition);
    let hop = txn
        .security()
        .insert_delegation_hop(NewDelegationHop {
            chain_id: DelegationChainId::new(&f.ids),
            hop_index: 0,
            principal_or_actor_id: "principal-a".to_owned(),
            run_id: Some(RunId::new(&f.ids)),
            capability_grant_ids: vec![1],
        })
        .await
        .expect_err("a hop must reference an existing run");
    assert_error(hop, ErrorCode::FailedPrecondition);
    txn.rollback().await.expect("rollback");
}
