//! Effects acceptance tests (EFF-002/003/004): preparation atomicity and
//! dedupe, the legal transition table, executor lease/fencing exclusivity over
//! 100 concurrent claimers, daemon-fence rejection, reconciliation, and the
//! never-auto-retry guarantee for `Unknown` effects.

use std::sync::Arc;

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{
    ActorId, AdapterId, CommandId, DaemonInstanceId, DecisionId, EffectId, EventCursor,
    EventStreamKey, PrincipalId, RunId, TaskId,
};
use domain::run::RecoveryDisposition;
use domain::run::RunState;
use errors::codes::ErrorCode;
use kernel_store::models::{NewEffect, NewRun, NewTask, OutboxEventRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

use effects::{
    AdapterBinding, ClaimOutcome, EFFECT_LEASE_MS, EffectContract, EffectEnv, ExecutorRef,
    ObservedOutcome, PrepareOutcome, PrepareRequest, ReconcilePlan, Resolution,
};

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";
const CLAIMED: EffectState = EffectState::Claimed;

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: Arc<DeterministicIds>,
    clock: Arc<TestClock>,
    principal: PrincipalId,
    actor: ActorId,
    epoch: u64,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp root");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join(DB_FILE),
                pool_max_connections: 8,
                busy_timeout_ms: 30_000,
            })
            .await
            .expect("store opens"),
        );
        let ids = Arc::new(DeterministicIds::new(SEED_MS));
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(ids.as_ref()))
            .await
            .expect("daemon fence acquired");
        let principal = PrincipalId::new(ids.as_ref());
        let actor = ActorId::new(ids.as_ref());
        Self {
            _dir: dir,
            store,
            ids,
            clock: Arc::new(TestClock::new(SEED_MS)),
            principal,
            actor,
            epoch: fence.epoch.0,
        }
    }

    fn context(&self) -> TxContext {
        TxContext {
            daemon_epoch: self.epoch,
            principal_id: self.principal,
            command_id: CommandId::new(self.ids.as_ref()),
            correlation_id: None,
        }
    }

    async fn write(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(self.context())
            .await
            .expect("write transaction opens")
    }

    async fn seed_run(&self) -> RunId {
        let task_id = TaskId::new(self.ids.as_ref());
        let run_id = RunId::new(self.ids.as_ref());
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("canonical stream key"),
            0,
        );
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: self.actor,
                task_kind: "effects".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id,
                session_id: None,
                parent_run_id: None,
                state: RunState::Running,
                recovery: RecoveryDisposition::Normal,
                loop_epoch: 0,
                step_sequence: 0,
                input_event_cursor: cursor,
                cancellation_epoch: 0,
                resolved_environment_id: None,
                agent_spec_id: None,
                agent_spec_version: None,
                agent_spec_digest: None,
                requested_profile: String::new(),
                workspace_uri: None,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed run");
        txn.commit().await.expect("seed run commits");
        run_id
    }

    fn prepare_request(&self, run_id: RunId, operation: &str) -> PrepareRequest {
        PrepareRequest {
            effect_id: EffectId::new(self.ids.as_ref()),
            run_id,
            step_sequence: 1,
            decision_id: DecisionId::new(self.ids.as_ref()),
            operation: operation.to_owned(),
            request_payload: b"request".to_vec(),
            adapter: AdapterBinding {
                adapter_id: AdapterId::new(self.ids.as_ref()),
                adapter_version: "1.0.0".to_owned(),
                adapter_digest: "digest".to_owned(),
            },
            contract: EffectContract {
                effect_class: EffectClass::ExternalMutation,
                idempotency: IdempotencySemantics::IdempotencyKeySupported,
                reconciliation: ReconciliationSemantics::ResultLookup,
                cancellation: effects::CancellationSemantics::Cooperative,
                compensation_capability: None,
            },
        }
    }

    fn env(&self) -> EffectEnv<'_> {
        EffectEnv {
            ids: self.ids.as_ref(),
            clock: self.clock.as_ref(),
            correlation_id: None,
            causation_id: None,
        }
    }

    async fn prepared(&self, operation: &str) -> (RunId, EffectId) {
        let run_id = self.seed_run().await;
        let request = self.prepare_request(run_id, operation);
        let effect_id = request.effect_id;
        let mut txn = self.write().await;
        let outcome = effects::prepare_effect(&mut *txn, &self.env(), request)
            .await
            .expect("prepare");
        assert!(matches!(outcome, PrepareOutcome::Prepared(_)));
        txn.commit().await.expect("prepare commits");
        (run_id, effect_id)
    }

    async fn effect(&self, effect_id: EffectId) -> kernel_store::models::EffectRow {
        let mut txn = self.write().await;
        let row = txn
            .effects()
            .get(effect_id)
            .await
            .expect("effect read")
            .expect("effect exists");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn events(&self) -> Vec<OutboxEventRow> {
        let mut txn = self.write().await;
        let rows = txn
            .streams()
            .scan_unpublished(1_000)
            .await
            .expect("outbox read");
        txn.rollback().await.expect("rollback");
        rows
    }

    async fn claim(&self, effect_id: EffectId, executor: &str) -> ClaimOutcome {
        let mut txn = self.write().await;
        let outcome = effects::claim(&mut *txn, &self.env(), effect_id, executor)
            .await
            .expect("claim");
        txn.commit().await.expect("claim commits");
        outcome
    }

    fn executor<'a>(&self, id: &'a str, token: u64) -> ExecutorRef<'a> {
        ExecutorRef {
            executor_id: id,
            fencing_token: token,
        }
    }
}

#[tokio::test]
async fn prepare_persists_record_and_stages_event() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    let row = h.effect(effect_id).await;
    assert_eq!(row.state, EffectState::Prepared);
    assert_eq!(row.operation, "fs.write");
    assert_eq!(row.executor_fencing_token, None);
    let events = h.events().await;
    let prepared = events
        .iter()
        .filter(|event| event.event_type == "EffectPrepared")
        .count();
    assert_eq!(prepared, 1);
    let event = events
        .iter()
        .find(|event| event.event_type == "EffectPrepared")
        .expect("EffectPrepared staged");
    assert_eq!(event.stream_key.to_string(), format!("effect/{effect_id}"));
}

#[tokio::test]
async fn duplicate_logical_identity_replays_committed_row() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let request = h.prepare_request(run_id, "fs.write");
    let mut txn = h.write().await;
    let first = effects::prepare_effect(&mut *txn, &h.env(), request.clone())
        .await
        .expect("first prepare");
    txn.commit().await.expect("commit");
    let PrepareOutcome::Prepared(first_row) = first else {
        panic!("first prepare inserts");
    };
    // A retried prepare reuses the same logical identity tuple.
    let mut retry = request.clone();
    retry.effect_id = EffectId::new(h.ids.as_ref());
    let mut txn = h.write().await;
    let second = effects::prepare_effect(&mut *txn, &h.env(), retry)
        .await
        .expect("replay prepare");
    txn.commit().await.expect("commit");
    let PrepareOutcome::Replayed(row) = second else {
        panic!("duplicate identity replays");
    };
    assert_eq!(row.effect_id, first_row.effect_id);
    let events = h.events().await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "EffectPrepared")
            .count(),
        1,
        "replay must not re-stage EffectPrepared"
    );
}

#[tokio::test]
async fn prepare_rolls_back_with_the_command() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let request = h.prepare_request(run_id, "fs.write");
    let effect_id = request.effect_id;
    let mut txn = h.write().await;
    effects::prepare_effect(&mut *txn, &h.env(), request)
        .await
        .expect("prepare");
    txn.rollback().await.expect("rollback");
    let mut txn = h.write().await;
    let row = txn.effects().get(effect_id).await.expect("read");
    txn.rollback().await.expect("rollback");
    assert!(row.is_none(), "rolled-back prepare leaves no record");
    let events = h.events().await;
    assert!(events.is_empty(), "rolled-back prepare stages no events");
}

#[tokio::test]
async fn no_dispatch_without_a_committed_prepared_record() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let request = h.prepare_request(run_id, "fs.write");
    let effect_id = request.effect_id;
    // Prepare inside a transaction that never commits.
    let mut txn = h.write().await;
    effects::prepare_effect(&mut *txn, &h.env(), request)
        .await
        .expect("prepare");
    txn.rollback().await.expect("rollback");
    // Claim cannot find the uncommitted record.
    let mut txn = h.write().await;
    let outcome = effects::claim(&mut *txn, &h.env(), effect_id, "executor-a").await;
    txn.rollback().await.expect("rollback");
    let err = outcome.expect_err("uncommitted record is not claimable");
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn illegal_transitions_are_rejected() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    // Prepared -> Committed skips the pipeline.
    let mut txn = h.write().await;
    let err = effects::commit(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("nobody", 1),
        None,
    )
    .await
    .expect_err("illegal transition rejected");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    txn.rollback().await.expect("rollback");
    // Terminal effects cannot transition again.
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("nobody", 1),
        None,
    )
    .await
    .expect_err("dispatched requires a claim");
    txn.rollback().await.expect("rollback");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn hundred_workers_single_current_claim() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    let barrier = Arc::new(Barrier::new(100));
    let mut handles = Vec::new();
    for worker in 0..100u32 {
        let store = h.store.clone();
        let ids = h.ids.clone();
        let clock = h.clock.clone();
        let epoch = h.epoch;
        let principal = h.principal;
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let context = TxContext {
                daemon_epoch: epoch,
                principal_id: principal,
                command_id: CommandId::new(ids.as_ref()),
                correlation_id: None,
            };
            let mut txn = store.begin_write(context).await.expect("write txn opens");
            let env = EffectEnv {
                ids: ids.as_ref(),
                clock: clock.as_ref(),
                correlation_id: None,
                causation_id: None,
            };
            let outcome =
                effects::claim(&mut *txn, &env, effect_id, &format!("executor-{worker}")).await;
            match outcome {
                Ok(ClaimOutcome::Claimed { fencing_token, .. }) => {
                    txn.commit().await.is_ok().then_some(fencing_token)
                }
                Ok(ClaimOutcome::Busy(_)) => None,
                Err(_) => {
                    let _ = txn.rollback().await;
                    None
                }
            }
        }));
    }
    let mut winners = Vec::new();
    for handle in handles {
        if let Some(token) = handle.await.expect("worker joins") {
            winners.push(token);
        }
    }
    assert_eq!(winners.len(), 1, "exactly one executor claims the effect");
    assert_eq!(winners[0], 1);
    let row = h.effect(effect_id).await;
    assert_eq!(row.state, CLAIMED);
    assert_eq!(row.executor_fencing_token, Some(1));
    let claims = h
        .events()
        .await
        .into_iter()
        .filter(|e| e.event_type == "EffectClaimed")
        .count();
    assert_eq!(claims, 1);
}

#[tokio::test]
async fn lease_expiry_reclaim_and_stale_executor_rejection() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    let ClaimOutcome::Claimed {
        fencing_token: token_a,
        ..
    } = h.claim(effect_id, "executor-a").await
    else {
        panic!("first claim wins");
    };
    // A live lease blocks a second claimant.
    match h.claim(effect_id, "executor-b").await {
        ClaimOutcome::Busy(_) => {}
        ClaimOutcome::Claimed { .. } => panic!("live claim must not hand off"),
    }
    // Lease expires; executor-b reclaims with a bumped fencing token.
    h.clock.advance(EFFECT_LEASE_MS + 1);
    let ClaimOutcome::Claimed {
        fencing_token: token_b,
        ..
    } = h.claim(effect_id, "executor-b").await
    else {
        panic!("expired lease is reclaimable");
    };
    assert_eq!(token_b, 2);
    // executor-a's late dispatch is diagnostic only.
    let mut txn = h.write().await;
    let err = effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", token_a),
        None,
    )
    .await
    .expect_err("stale executor is rejected");
    assert_eq!(err.code(), ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
    // The new claimant dispatches authoritatively.
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-b", token_b),
        None,
    )
    .await
    .expect("new claimant dispatches");
    txn.commit().await.expect("dispatch commits");
    let row = h.effect(effect_id).await;
    assert_eq!(row.state, EffectState::Dispatched);
    assert_eq!(row.executor_id.as_deref(), Some("executor-b"));
    assert_eq!(row.executor_fencing_token, Some(2));
}

#[tokio::test]
async fn daemon_fence_change_rejects_old_executor() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    let ClaimOutcome::Claimed { fencing_token, .. } = h.claim(effect_id, "executor-a").await else {
        panic!("claim wins");
    };
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", fencing_token),
        None,
    )
    .await
    .expect("dispatched");
    txn.commit().await.expect("dispatch commits");
    // A restarted daemon re-acquires the fence under a new epoch; the
    // pre-restart executor's results cannot advance the effect.
    let new_fence = h
        .store
        .acquire_daemon_fence(DaemonInstanceId::new(h.ids.as_ref()))
        .await
        .expect("daemon fence re-acquired");
    assert!(new_fence.epoch.0 > h.epoch);
    let context = TxContext {
        daemon_epoch: new_fence.epoch.0,
        principal_id: h.principal,
        command_id: CommandId::new(h.ids.as_ref()),
        correlation_id: None,
    };
    let mut txn = h.store.begin_write(context).await.expect("write txn opens");
    let err = effects::acknowledge(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", fencing_token),
        Some("result".to_owned()),
    )
    .await
    .expect_err("stale daemon epoch rejected");
    assert_eq!(err.code(), ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn reconcile_settles_dispatched_effect() {
    let h = Harness::new().await;
    let (_run, effect_id) = h.prepared("fs.write").await;
    let ClaimOutcome::Claimed { fencing_token, .. } = h.claim(effect_id, "executor-a").await else {
        panic!("claim wins");
    };
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", fencing_token),
        None,
    )
    .await
    .expect("dispatched");
    txn.commit().await.expect("dispatch commits");
    let row = h.effect(effect_id).await;
    let plan = effects::reconcile_plan(
        &row,
        ObservedOutcome::Succeeded {
            result_ref: "artifact://r".to_owned(),
        },
    );
    let mut txn = h.write().await;
    let settled = effects::apply_observed(&mut *txn, &h.env(), effect_id, plan)
        .await
        .expect("reconcile applies");
    txn.commit().await.expect("reconcile commits");
    assert_eq!(settled.state, EffectState::Committed);
    let types: Vec<_> = h.events().await.into_iter().map(|e| e.event_type).collect();
    assert!(types.contains(&"EffectReconciled".to_owned()));
}

#[tokio::test]
async fn unreconcilable_dispatch_parks_unknown_and_resolves() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let mut request = h.prepare_request(run_id, "http.post");
    request.contract = EffectContract::unknown();
    let effect_id = request.effect_id;
    let mut txn = h.write().await;
    effects::prepare_effect(&mut *txn, &h.env(), request)
        .await
        .expect("prepare");
    txn.commit().await.expect("commit");
    let ClaimOutcome::Claimed { fencing_token, .. } = h.claim(effect_id, "executor-a").await else {
        panic!("claim wins");
    };
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", fencing_token),
        None,
    )
    .await
    .expect("dispatched");
    txn.commit().await.expect("dispatch commits");
    let row = h.effect(effect_id).await;
    // Provider has no record and the contract is not safely redispatchable.
    let plan = effects::reconcile_plan(&row, ObservedOutcome::NotFound);
    assert_eq!(plan, ReconcilePlan::Unknown);
    let mut txn = h.write().await;
    effects::apply_observed(&mut *txn, &h.env(), effect_id, plan)
        .await
        .expect("unknown parks");
    txn.commit().await.expect("commit");
    // The reconciler must never reopen an Unknown effect.
    let row = h.effect(effect_id).await;
    let plan = effects::reconcile_plan(&row, ObservedOutcome::NotFound);
    let mut txn = h.write().await;
    let row = effects::apply_observed(&mut *txn, &h.env(), effect_id, plan)
        .await
        .expect("no-op on unknown");
    txn.commit().await.expect("commit");
    assert_eq!(row.state, EffectState::Unknown);
    // mark_failed settles the effect.
    let mut txn = h.write().await;
    let resolved = effects::apply_resolution(
        &mut *txn,
        &h.env(),
        effect_id,
        Resolution::MarkFailed,
        None,
        "operator confirms failure",
    )
    .await
    .expect("resolution applies");
    txn.commit().await.expect("commit");
    assert_eq!(resolved.state, EffectState::Failed);
    let reconciled = h
        .events()
        .await
        .into_iter()
        .filter(|e| e.event_type == "EffectReconciled")
        .count();
    assert_eq!(reconciled, 1, "settlement stages EffectReconciled");
}

#[tokio::test]
async fn resolution_retry_reprepares_same_operation_identity() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let mut request = h.prepare_request(run_id, "http.post");
    request.contract = EffectContract::unknown();
    let effect_id = request.effect_id;
    let operation = request.operation.clone();
    let decision_id = request.decision_id;
    let mut txn = h.write().await;
    effects::prepare_effect(&mut *txn, &h.env(), request)
        .await
        .expect("prepare");
    txn.commit().await.expect("commit");
    let ClaimOutcome::Claimed { fencing_token, .. } = h.claim(effect_id, "executor-a").await else {
        panic!("claim wins");
    };
    let mut txn = h.write().await;
    effects::mark_dispatched(
        &mut *txn,
        &h.env(),
        effect_id,
        &h.executor("executor-a", fencing_token),
        None,
    )
    .await
    .expect("dispatched");
    effects::mark_unknown(&mut *txn, &h.env(), effect_id, None)
        .await
        .expect("parks unknown");
    txn.commit().await.expect("commit");
    let mut txn = h.write().await;
    let row = effects::apply_resolution(
        &mut *txn,
        &h.env(),
        effect_id,
        Resolution::RetryAcceptingDuplicateRisk,
        None,
        "operator accepts duplicate risk",
    )
    .await
    .expect("retry resolution");
    txn.commit().await.expect("commit");
    assert_eq!(row.state, EffectState::Prepared);
    assert_eq!(row.operation, operation);
    assert_eq!(row.decision_id, decision_id, "same operation identity");
    // The re-prepared effect is claimable again under a bumped token.
    let ClaimOutcome::Claimed { fencing_token, .. } = h.claim(effect_id, "executor-b").await else {
        panic!("re-prepared effect is claimable");
    };
    assert_eq!(fencing_token, 2);
}

#[tokio::test]
async fn mock_store_satisfies_the_effect_repo_contract() {
    let store = testkit::store::MockStore::new();
    let ids = DeterministicIds::new(SEED_MS);
    store
        .set_daemon_epoch(7, DaemonInstanceId::new(&ids))
        .expect("daemon epoch");
    let task_id = TaskId::new(&ids);
    let run_id = RunId::new(&ids);
    let mut txn = store
        .begin_write(TxContext {
            daemon_epoch: 7,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn");
    txn.tasks()
        .insert(NewTask {
            task_id,
            session_id: None,
            created_by_actor_id: ActorId::new(&ids),
            task_kind: "effects".to_owned(),
            payload: Vec::new(),
            created_at_ms: SEED_MS,
        })
        .await
        .expect("task");
    txn.runs()
        .insert(NewRun {
            run_id,
            task_id,
            session_id: None,
            parent_run_id: None,
            state: RunState::Running,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: EventCursor::new(
                EventStreamKey::new(format!("run/{run_id}")).expect("stream"),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: String::new(),
            workspace_uri: None,
            created_at_ms: SEED_MS,
        })
        .await
        .expect("run");
    let effect_id = EffectId::new(&ids);
    txn.effects()
        .insert(NewEffect {
            effect_id,
            run_id,
            step_sequence: 1,
            decision_id: DecisionId::new(&ids),
            operation: "op".to_owned(),
            request_hash: "h".to_owned(),
            request_payload: Vec::new(),
            effect_class: EffectClass::ReadOnly,
            idempotency_semantics: IdempotencySemantics::NaturallyIdempotent,
            reconciliation_semantics: ReconciliationSemantics::ResultLookup,
            cancellation_semantics: "before_dispatch".to_owned(),
            compensation_capability: None,
            adapter_id: AdapterId::new(&ids),
            adapter_version: "1".to_owned(),
            adapter_digest: "d".to_owned(),
            state: EffectState::Prepared,
            created_at_ms: SEED_MS,
        })
        .await
        .expect("effect");
    let token = txn
        .effects()
        .claim(effect_id, "exec", 7, SEED_MS + 1_000, SEED_MS)
        .await
        .expect("claim");
    assert_eq!(token, Some(1));
    let busy = txn
        .effects()
        .claim(effect_id, "exec-2", 7, SEED_MS + 1_000, SEED_MS)
        .await
        .expect("claim");
    assert_eq!(busy, None, "live claim blocks a second executor");
    txn.commit().await.expect("commit");
}
