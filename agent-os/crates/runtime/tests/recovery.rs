//! Startup recovery acceptance tests (R6.1-R6.6, P4): enumeration of
//! non-terminal runs, matrix classification over the real SQLite store for
//! effects, timers, adapter bindings, and approval expiry, fail-closed
//! unmapped combinations, disposition persistence, and the guarantee that an
//! ambiguous effect never leaves a run resumable.

use std::sync::Arc;

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::generated::contract;
use domain::ids::{
    AdapterId, AgentSpecId, ApprovalRequestId, ConfigGenerationId, DaemonInstanceId, DecisionId,
    EnvironmentId, EventCursor, EventId, EventStreamKey, PrincipalId, RunId, TaskId, TimerId,
};
use domain::provider::IdProvider;
use domain::resource::{DependencyCondition, TimerState, WorkspaceAccessMode};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::ApprovalState;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelStore;
use kernel_store::models::{
    NewApprovalRequest, NewEffect, NewResolvedBinding, NewResolvedEnvironment, NewRun, NewTask,
    NewTimer, OutboxEventRow, RunCas, RunPatch, RunRow, TimerPatch,
};
use kernel_store::{KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use run_graph::graph::add_dependency;
use runtime::recovery::reconstruct;
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";
const ADAPTER_VERSION: &str = "1.0.0";
const ADAPTER_DIGEST: &str = "adapter-digest-a";
const EFFECT_PAYLOAD_MARKER: &[u8] = b"do-not-echo-effect-payload-4f2c";

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: Arc<DeterministicIds>,
    clock: Arc<TestClock>,
    principal: PrincipalId,
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
        Self {
            _dir: dir,
            store,
            ids,
            clock: Arc::new(TestClock::new(SEED_MS)),
            principal,
            epoch: fence.epoch.0,
        }
    }

    fn context(&self) -> TxContext {
        TxContext {
            daemon_epoch: self.epoch,
            principal_id: self.principal,
            command_id: domain::ids::CommandId::new(self.ids.as_ref()),
            correlation_id: None,
        }
    }

    async fn write(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(self.context())
            .await
            .expect("write transaction opens")
    }

    async fn seed_task(&self, task_id: TaskId) {
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: domain::ids::ActorId::new(self.ids.as_ref()),
                task_kind: "recovery".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.graph().ensure_head(task_id).await.expect("seed head");
        txn.commit().await.expect("seed task commits");
    }

    async fn seed_run_at(&self, task: TaskId, state: RunState, created_at_ms: i64) -> RunId {
        let run_id = RunId::new(self.ids.as_ref());
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("canonical stream key"),
            0,
        );
        let mut txn = self.write().await;
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id: task,
                session_id: None,
                parent_run_id: None,
                state,
                recovery: RecoveryDisposition::Normal,
                loop_epoch: 0,
                step_sequence: 0,
                input_event_cursor: cursor,
                cancellation_epoch: 0,
                resolved_environment_id: None,
                created_at_ms,
            })
            .await
            .expect("seed run");
        txn.commit().await.expect("seed run commits");
        run_id
    }

    async fn seed_run(&self, task: TaskId, state: RunState) -> RunId {
        self.seed_run_at(task, state, SEED_MS).await
    }

    /// Test-only scaffolding: production reaches `Ready`/`WaitingChild` through
    /// the binder or loop transitions this task does not own; the repository
    /// CAS lets recovery be tested in isolation (D4 precedent).
    async fn force_state(&self, run_id: RunId, state: RunState) {
        let mut txn = self.write().await;
        let current = txn
            .runs()
            .get(run_id)
            .await
            .expect("run read")
            .expect("seeded run exists");
        let updated = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: current.run_revision,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    state: Some(state),
                    ..RunPatch::default()
                },
            )
            .await
            .expect("state patch applies");
        assert!(updated, "state patch lands");
        txn.commit().await.expect("state patch commits");
    }

    /// Seeds the frozen environment and binds one adapter for `run_id`.
    async fn seed_environment(&self, run_id: RunId, adapter: AdapterId) -> EnvironmentId {
        let environment_id = EnvironmentId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.environments()
            .insert_environment(NewResolvedEnvironment {
                environment_id,
                run_id,
                agent_spec_id: AgentSpecId::new(self.ids.as_ref()),
                agent_spec_version: "1.0.0".to_owned(),
                agent_spec_digest: "spec-digest".to_owned(),
                agent_loop_id: "agent-loop".to_owned(),
                agent_loop_version: "1.0.0".to_owned(),
                agent_loop_digest: "loop-digest".to_owned(),
                config_generation_id: ConfigGenerationId::new(self.ids.as_ref()),
                workspace_uri: None,
                workspace_base_revision: None,
                workspace_mode: WorkspaceAccessMode::ReadOnly,
                model_provider: None,
                model_id: None,
                model_parameters: None,
                kernel_version: "0.1.0".to_owned(),
                protocol_versions: Vec::new(),
                capability_grant_ids: Vec::new(),
                approval_request_ids: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed environment");
        txn.environments()
            .insert_bindings(
                environment_id,
                vec![NewResolvedBinding {
                    port_id: "model".to_owned(),
                    adapter_id: adapter,
                    adapter_version: ADAPTER_VERSION.to_owned(),
                    adapter_digest: ADAPTER_DIGEST.to_owned(),
                    capabilities: Vec::new(),
                }],
            )
            .await
            .expect("seed bindings");
        let current = txn
            .runs()
            .get(run_id)
            .await
            .expect("run read")
            .expect("seeded run exists");
        let updated = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: current.run_revision,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    resolved_environment_id: Some(environment_id),
                    ..RunPatch::default()
                },
            )
            .await
            .expect("environment patch applies");
        assert!(updated, "environment patch lands");
        txn.commit().await.expect("environment commits");
        environment_id
    }

    async fn seed_effect_with_adapter(
        &self,
        run_id: RunId,
        state: EffectState,
        reconciliation: ReconciliationSemantics,
        idempotency: IdempotencySemantics,
        adapter: AdapterId,
    ) -> domain::ids::EffectId {
        let effect_id = domain::ids::EffectId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.effects()
            .insert(NewEffect {
                effect_id,
                run_id,
                step_sequence: 0,
                decision_id: DecisionId::new(self.ids.as_ref()),
                operation: "invoke".to_owned(),
                request_hash: "request-hash".to_owned(),
                request_payload: EFFECT_PAYLOAD_MARKER.to_vec(),
                effect_class: EffectClass::ExternalMutation,
                idempotency_semantics: idempotency,
                reconciliation_semantics: reconciliation,
                cancellation_semantics: "compensatable".to_owned(),
                compensation_capability: None,
                adapter_id: adapter,
                adapter_version: ADAPTER_VERSION.to_owned(),
                adapter_digest: ADAPTER_DIGEST.to_owned(),
                state,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed effect");
        txn.commit().await.expect("effect commits");
        effect_id
    }

    async fn seed_effect(
        &self,
        run_id: RunId,
        state: EffectState,
        reconciliation: ReconciliationSemantics,
        idempotency: IdempotencySemantics,
    ) -> domain::ids::EffectId {
        let adapter = AdapterId::new(self.ids.as_ref());
        self.seed_effect_with_adapter(run_id, state, reconciliation, idempotency, adapter)
            .await
    }

    async fn seed_timer(
        &self,
        run_id: RunId,
        state: TimerState,
        claim_daemon_epoch: Option<u64>,
    ) -> TimerId {
        let timer_id = TimerId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.timers()
            .insert(NewTimer {
                timer_id,
                run_id: Some(run_id),
                timer_kind: "wait".to_owned(),
                payload: Vec::new(),
                due_at_ms: SEED_MS - 1,
                state: TimerState::Scheduled,
                version: 0,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed timer");
        if state != TimerState::Scheduled {
            let updated = txn
                .timers()
                .cas_transition(
                    timer_id,
                    TimerState::Scheduled,
                    0,
                    TimerPatch {
                        state: Some(state),
                        claim_daemon_epoch,
                        ..TimerPatch::default()
                    },
                )
                .await
                .expect("timer transition");
            assert!(updated, "timer transition lands");
        }
        txn.commit().await.expect("timer commits");
        timer_id
    }

    async fn seed_approval(
        &self,
        run_id: RunId,
        expires_at_ms: i64,
        created_at_ms: i64,
    ) -> ApprovalRequestId {
        let request_id = ApprovalRequestId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.security()
            .insert_approval_request(NewApprovalRequest {
                request_id,
                request_digest: format!("digest-{request_id}"),
                principal_id: self.principal,
                actor_id: domain::ids::ActorId::new(self.ids.as_ref()),
                run_id: Some(run_id),
                operation: "invoke".to_owned(),
                target_resource: None,
                capability_ids: Vec::new(),
                extension_bundle_digest: None,
                config_generation_digest: None,
                expires_at_ms,
                nonce: format!("nonce-{request_id}"),
                state: ApprovalState::Pending,
                created_at_ms,
                resolved_at_ms: None,
            })
            .await
            .expect("seed approval");
        txn.commit().await.expect("approval commits");
        request_id
    }

    async fn add_edge(
        &self,
        source: RunId,
        target: RunId,
        condition: DependencyCondition,
        expected_revision: u64,
    ) {
        let mut txn = self.write().await;
        add_dependency(
            txn.as_mut(),
            source,
            target,
            condition,
            expected_revision,
            SEED_MS,
        )
        .await
        .expect("edge commits");
        txn.commit().await.expect("edge transaction commits");
    }

    async fn run(&self, run_id: RunId) -> Option<RunRow> {
        let mut txn = self.write().await;
        let row = txn.runs().get(run_id).await.expect("run read");
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
}

fn report_of(report: &runtime::recovery::RecoveryReport, run_id: RunId) -> RecoveryDisposition {
    report
        .dispositions
        .iter()
        .find(|(candidate, _)| *candidate == run_id)
        .map(|(_, disposition)| *disposition)
        .expect("run is reported")
}

fn stream_events_for(events: &[OutboxEventRow], run_id: RunId) -> Vec<&OutboxEventRow> {
    events
        .iter()
        .filter(|event| event.stream_key.as_str() == format!("run/{run_id}"))
        .collect()
}

#[tokio::test]
async fn running_run_without_effects_recovers_and_persists_the_disposition() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Running).await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(report.examined, 1);
    assert_eq!(
        report.dispositions,
        vec![(run, RecoveryDisposition::Recovering)]
    );

    let row = harness.run(run).await.expect("run persists");
    assert_eq!(row.state, RunState::Running, "recovery never changes state");
    assert_eq!(row.recovery, RecoveryDisposition::Recovering);
    assert_eq!(row.run_revision, 1, "one disposition patch");

    let events = harness.events().await;
    let staged = stream_events_for(&events, run);
    assert_eq!(staged.len(), 1, "one disposition event");
    assert_eq!(staged[0].event_type, "RunRecoveryDispositionChanged");
    let payload =
        contract::AgentRun::decode(staged[0].payload.as_slice()).expect("AgentRun payload");
    assert_eq!(payload.run_id, run.to_string());
    assert_eq!(payload.state, RunState::Running.to_wire());
    assert_eq!(payload.recovery, RecoveryDisposition::Recovering.to_wire());
    assert_eq!(payload.run_revision, 1);
}

#[tokio::test]
async fn unknown_effect_blocks_the_run_and_never_leaves_it_resumable() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Running).await;
    harness
        .seed_effect(
            run,
            EffectState::Unknown,
            ReconciliationSemantics::Impossible,
            IdempotencySemantics::NotIdempotent,
        )
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report.dispositions,
        vec![(run, RecoveryDisposition::BlockedUnknownEffect)]
    );

    let row = harness.run(run).await.expect("run persists");
    assert_eq!(row.state, RunState::Running);
    assert_eq!(row.recovery, RecoveryDisposition::BlockedUnknownEffect);
    assert_eq!(row.run_revision, 1);

    // Even a scaffolded `Ready` run cannot be claimed while blocked (P4).
    harness.force_state(run, RunState::Ready).await;
    let error = {
        let mut txn = harness.write().await;
        let result = runtime::claim::claim_with_ids(
            txn.as_mut(),
            run,
            "worker-1".to_owned(),
            60_000,
            SEED_MS,
            harness.epoch,
            harness.ids.as_ref(),
        )
        .await;
        txn.rollback().await.expect("rollback");
        result.expect_err("blocked disposition rejects the claim")
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn prepared_effect_without_a_matching_binding_is_blocked() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Running).await;
    let bound_adapter = AdapterId::new(harness.ids.as_ref());
    harness.seed_environment(run, bound_adapter).await;
    let unbound_adapter = AdapterId::new(harness.ids.as_ref());
    harness
        .seed_effect_with_adapter(
            run,
            EffectState::Prepared,
            ReconciliationSemantics::StatusLookup,
            IdempotencySemantics::NaturallyIdempotent,
            unbound_adapter,
        )
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report.dispositions,
        vec![(run, RecoveryDisposition::BlockedMissingResource)]
    );

    let row = harness.run(run).await.expect("run persists");
    assert_eq!(row.state, RunState::Running);
    assert_eq!(row.recovery, RecoveryDisposition::BlockedMissingResource);
    assert_eq!(row.run_revision, 1);
}

#[tokio::test]
async fn prepared_effect_without_a_frozen_environment_is_blocked() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Running).await;
    harness
        .seed_effect(
            run,
            EffectState::Prepared,
            ReconciliationSemantics::StatusLookup,
            IdempotencySemantics::NaturallyIdempotent,
        )
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report.dispositions,
        vec![(run, RecoveryDisposition::BlockedMissingResource)]
    );
}

#[tokio::test]
async fn repeated_restarts_report_the_same_disposition_without_rewriting() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Running).await;

    let first = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("first reconstruction succeeds");
    let second = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("second reconstruction succeeds");
    assert_eq!(first, second);
    assert_eq!(
        first.dispositions,
        vec![(run, RecoveryDisposition::Recovering)]
    );

    let row = harness.run(run).await.expect("run persists");
    assert_eq!(
        row.run_revision, 1,
        "an unchanged disposition is not rewritten"
    );
    assert_eq!(
        stream_events_for(&harness.events().await, run).len(),
        1,
        "no duplicate audit event"
    );
}

#[tokio::test]
async fn unmapped_combination_fails_closed_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Created).await;
    let effect = harness
        .seed_effect(
            run,
            EffectState::Prepared,
            ReconciliationSemantics::StatusLookup,
            IdempotencySemantics::NaturallyIdempotent,
        )
        .await;

    let error = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect_err("an unmapped pair aborts startup");
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(error.message().contains(&run.to_string()));
    assert!(error.message().contains(&effect.to_string()));
    assert!(
        !error.message().contains("do-not-echo"),
        "payload bytes are never rendered into errors"
    );

    let row = harness.run(run).await.expect("run persists");
    assert_eq!(row.recovery, RecoveryDisposition::Normal);
    assert_eq!(row.run_revision, 0, "fail-closed writes nothing");
    assert!(harness.events().await.is_empty(), "no partial events");
}

#[tokio::test]
async fn terminal_runs_are_never_enumerated() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let completed = harness
        .seed_run_at(task, RunState::Completed, SEED_MS)
        .await;
    let failed = harness
        .seed_run_at(task, RunState::Failed, SEED_MS + 1)
        .await;
    let cancelled = harness
        .seed_run_at(task, RunState::Cancelled, SEED_MS + 2)
        .await;
    let running = harness
        .seed_run_at(task, RunState::Running, SEED_MS + 3)
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(report.examined, 1, "terminal runs are excluded");
    assert_eq!(
        report.dispositions,
        vec![(running, RecoveryDisposition::Recovering)]
    );

    for terminal in [completed, failed, cancelled] {
        let row = harness.run(terminal).await.expect("terminal run persists");
        assert_eq!(row.recovery, RecoveryDisposition::Normal);
        assert_eq!(row.run_revision, 0);
        assert!(stream_events_for(&harness.events().await, terminal).is_empty());
    }
}

#[tokio::test]
async fn classification_follows_the_recovery_matrix() {
    struct Case {
        run_state: RunState,
        effect: Option<(EffectState, ReconciliationSemantics, IdempotencySemantics)>,
        expected: RecoveryDisposition,
    }

    // Coverage is complete for the matrix rows: effects, timers, adapter
    // bindings, and the `WaitingHuman` approval-expiry refinement (the cases
    // below seed no approval, so the pending-approval `Normal` row applies;
    // the dedicated expiry test covers the `RequiresHumanDecision` row).
    let cases = vec![
        Case {
            run_state: RunState::Created,
            effect: None,
            expected: RecoveryDisposition::Normal,
        },
        Case {
            run_state: RunState::Ready,
            effect: None,
            expected: RecoveryDisposition::Normal,
        },
        Case {
            run_state: RunState::Running,
            effect: None,
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::WaitingTool,
            effect: None,
            expected: RecoveryDisposition::Normal,
        },
        Case {
            // Pending-approval row: no approval row exists, so the run stays
            // `Normal`; an expired latest approval selects
            // `RequiresHumanDecision` (see the dedicated expiry test).
            run_state: RunState::WaitingHuman,
            effect: None,
            expected: RecoveryDisposition::Normal,
        },
        Case {
            run_state: RunState::Suspended,
            effect: None,
            expected: RecoveryDisposition::RequiresHumanDecision,
        },
        Case {
            run_state: RunState::Cancelling,
            effect: None,
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Prepared,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::Normal,
        },
        Case {
            run_state: RunState::WaitingTool,
            effect: Some((
                EffectState::Prepared,
                ReconciliationSemantics::ResultLookup,
                IdempotencySemantics::UnknownIdempotency,
            )),
            expected: RecoveryDisposition::Normal,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Claimed,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::IdempotencyKeySupported,
            )),
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::Cancelling,
            effect: Some((
                EffectState::Prepared,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::Cancelling,
            effect: Some((
                EffectState::Claimed,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Acknowledged,
                ReconciliationSemantics::ResultLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::Recovering,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NotIdempotent,
            )),
            expected: RecoveryDisposition::NeedsReconciliation,
        },
        Case {
            run_state: RunState::Cancelling,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::DeterministicInspection,
                IdempotencySemantics::UnknownIdempotency,
            )),
            expected: RecoveryDisposition::NeedsReconciliation,
        },
        Case {
            run_state: RunState::WaitingTool,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::Impossible,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::NeedsReconciliation,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::UnknownReconciliation,
                IdempotencySemantics::IdempotencyKeySupported,
            )),
            expected: RecoveryDisposition::NeedsReconciliation,
        },
        Case {
            run_state: RunState::Running,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::Impossible,
                IdempotencySemantics::NotIdempotent,
            )),
            expected: RecoveryDisposition::BlockedUnknownEffect,
        },
        Case {
            run_state: RunState::Cancelling,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::UnknownReconciliation,
                IdempotencySemantics::UnknownIdempotency,
            )),
            expected: RecoveryDisposition::BlockedUnknownEffect,
        },
        Case {
            run_state: RunState::WaitingTool,
            effect: Some((
                EffectState::Unknown,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::BlockedUnknownEffect,
        },
        Case {
            run_state: RunState::Suspended,
            effect: Some((
                EffectState::Prepared,
                ReconciliationSemantics::StatusLookup,
                IdempotencySemantics::NaturallyIdempotent,
            )),
            expected: RecoveryDisposition::RequiresHumanDecision,
        },
        Case {
            run_state: RunState::Suspended,
            effect: Some((
                EffectState::Dispatched,
                ReconciliationSemantics::Impossible,
                IdempotencySemantics::NotIdempotent,
            )),
            expected: RecoveryDisposition::BlockedUnknownEffect,
        },
        Case {
            run_state: RunState::Suspended,
            effect: Some((
                EffectState::Unknown,
                ReconciliationSemantics::Impossible,
                IdempotencySemantics::NotIdempotent,
            )),
            expected: RecoveryDisposition::BlockedUnknownEffect,
        },
    ];

    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let mut expected = Vec::with_capacity(cases.len());
    for (index, case) in cases.iter().enumerate() {
        let run = harness
            .seed_run_at(task, case.run_state, SEED_MS + index as i64)
            .await;
        if let Some((state, reconciliation, idempotency)) = case.effect {
            let adapter = AdapterId::new(harness.ids.as_ref());
            harness.seed_environment(run, adapter).await;
            harness
                .seed_effect_with_adapter(run, state, reconciliation, idempotency, adapter)
                .await;
        }
        expected.push((run, case.expected));
    }

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("matrix-covered combinations reconstruct");
    assert_eq!(report.examined, cases.len());
    assert_eq!(report.dispositions, expected);
}

#[tokio::test]
async fn waiting_child_depends_on_its_declared_conditions() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let done_child = harness
        .seed_run_at(task, RunState::Completed, SEED_MS)
        .await;
    let parent_ready = harness
        .seed_run_at(task, RunState::Created, SEED_MS + 1)
        .await;
    harness
        .add_edge(
            done_child,
            parent_ready,
            DependencyCondition::CompletedSuccessfully,
            0,
        )
        .await;
    harness
        .force_state(parent_ready, RunState::WaitingChild)
        .await;

    let busy_child = harness
        .seed_run_at(task, RunState::Running, SEED_MS + 2)
        .await;
    let parent_waiting = harness
        .seed_run_at(task, RunState::Created, SEED_MS + 3)
        .await;
    harness
        .add_edge(
            busy_child,
            parent_waiting,
            DependencyCondition::CompletedSuccessfully,
            1,
        )
        .await;
    harness
        .force_state(parent_waiting, RunState::WaitingChild)
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report_of(&report, parent_ready),
        RecoveryDisposition::Recovering,
        "every declared condition is satisfied"
    );
    assert_eq!(
        report_of(&report, busy_child),
        RecoveryDisposition::Recovering,
        "children are recovered independently"
    );
    assert_eq!(
        report_of(&report, parent_waiting),
        RecoveryDisposition::Normal,
        "the parent stays waiting while a child is non-terminal"
    );
}

#[tokio::test]
async fn stale_claimed_timers_hold_the_run_out_of_normal() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let scheduled = harness
        .seed_run_at(task, RunState::WaitingTool, SEED_MS)
        .await;
    harness
        .seed_timer(scheduled, TimerState::Scheduled, None)
        .await;
    let stale = harness
        .seed_run_at(task, RunState::WaitingTool, SEED_MS + 1)
        .await;
    harness.seed_timer(stale, TimerState::Claimed, None).await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report.dispositions,
        vec![
            (scheduled, RecoveryDisposition::Normal),
            (stale, RecoveryDisposition::Recovering),
        ]
    );

    let scheduled_row = harness.run(scheduled).await.expect("run persists");
    assert_eq!(
        scheduled_row.run_revision, 0,
        "an overdue timer is claimable"
    );
    let stale_row = harness.run(stale).await.expect("run persists");
    assert_eq!(stale_row.recovery, RecoveryDisposition::Recovering);
    assert_eq!(stale_row.run_revision, 1);
}

#[tokio::test]
async fn waiting_human_approval_expiry_selects_requires_human_decision() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let expired = harness
        .seed_run_at(task, RunState::WaitingHuman, SEED_MS)
        .await;
    harness.seed_approval(expired, SEED_MS, SEED_MS - 10).await;

    let pending = harness
        .seed_run_at(task, RunState::WaitingHuman, SEED_MS + 1)
        .await;
    harness
        .seed_approval(pending, SEED_MS + 60_000, SEED_MS)
        .await;

    // A renewed live approval supersedes an older expired one.
    let renewed = harness
        .seed_run_at(task, RunState::WaitingHuman, SEED_MS + 2)
        .await;
    harness
        .seed_approval(renewed, SEED_MS - 1, SEED_MS - 10)
        .await;
    harness
        .seed_approval(renewed, SEED_MS + 60_000, SEED_MS + 1)
        .await;

    let report = reconstruct(harness.store.as_ref(), harness.clock.clone())
        .await
        .expect("reconstruction succeeds");
    assert_eq!(
        report.dispositions,
        vec![
            (expired, RecoveryDisposition::RequiresHumanDecision),
            (pending, RecoveryDisposition::Normal),
            (renewed, RecoveryDisposition::Normal),
        ]
    );

    let expired_row = harness.run(expired).await.expect("run persists");
    assert_eq!(
        expired_row.state,
        RunState::WaitingHuman,
        "recovery never changes state"
    );
    assert_eq!(
        expired_row.recovery,
        RecoveryDisposition::RequiresHumanDecision
    );
    assert_eq!(expired_row.run_revision, 1, "one disposition patch");
    let events = harness.events().await;
    let staged = stream_events_for(&events, expired);
    assert_eq!(staged.len(), 1, "one disposition event");
    assert_eq!(staged[0].event_type, "RunRecoveryDispositionChanged");

    for unchanged in [pending, renewed] {
        let row = harness.run(unchanged).await.expect("run persists");
        assert_eq!(row.recovery, RecoveryDisposition::Normal, "{unchanged}");
        assert_eq!(
            row.run_revision, 0,
            "a live approval is not rewritten: {unchanged}"
        );
        assert!(
            stream_events_for(&events, unchanged).is_empty(),
            "no audit event for {unchanged}"
        );
    }
}

#[tokio::test]
async fn claims_draw_tokens_and_event_ids_from_the_injected_provider() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;

    let provider = DeterministicIds::new(SEED_MS);
    let expected = DeterministicIds::new(SEED_MS);
    let token_uuid = expected.new_uuid_v7();
    let claimed_event = EventId::new(&expected);
    let started_event = EventId::new(&expected);

    let mut txn = harness.write().await;
    runtime::claim::claim_with_ids(
        txn.as_mut(),
        run,
        "worker-1".to_owned(),
        30_000,
        SEED_MS,
        harness.epoch,
        &provider,
    )
    .await
    .expect("a ready run is claimed");
    txn.commit().await.expect("claim commits");

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(
        persisted.claim_token,
        Some((token_uuid.as_u128() & i64::MAX as u128) as u64),
        "the token is the injected provider's masked UUIDv7"
    );

    let events = harness.events().await;
    let staged: Vec<EventId> = events.iter().map(|event| event.event_id).collect();
    assert_eq!(
        staged,
        vec![claimed_event, started_event],
        "event ids come from the injected provider in staging order"
    );
}
