//! Contract test for the in-memory `KernelStore` mock.
//!
//! The mock is the second implementation of the port. These tests prove the
//! contract is implementable: atomic visibility on commit, no residue on drop,
//! CAS conditions that reject stale expectations, idempotency replay,
//! contiguous stream allocation, and the daemon-epoch assertion at write begin.

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{
    ActorId, AdapterId, CommandId, DaemonInstanceId, DecisionId, EffectId, EventCursor, EventId,
    EventStreamKey, IdempotencyKey, PrincipalId, RunId, SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::ErrorCode;
use kernel_store::{
    EffectPatch, KernelStore, NewEffect, NewIdempotencyRecord, NewOutboxEvent, NewRun, NewSession,
    NewTask, RunCas, RunPatch, TxContext,
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
