//! Claim acceptance tests (R4.2-R4.6, P2): readiness, recovery, and live-claim
//! gates, expired-claim reclamation, fence persistence, event staging, and the
//! 100-way single-winner race over the real SQLite kernel store.

use std::str::FromStr;
use std::sync::Arc;

use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use domain::generated::contract;
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, IdempotencyKey, PrincipalId,
    RunId, TaskId,
};
use domain::resource::DependencyCondition;
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{ClaimPatch, NewRun, NewTask, OutboxEventRow, RunCas, RunPatch, RunRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use run_graph::graph::add_dependency;
use runtime::claim::claim;
use runtime::{CMD_CLAIM_READY_RUN, RuntimeDeps};
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";

const PERSISTED_STATES: [RunState; 11] = [
    RunState::Created,
    RunState::Ready,
    RunState::Running,
    RunState::WaitingTool,
    RunState::WaitingChild,
    RunState::WaitingHuman,
    RunState::Suspended,
    RunState::Cancelling,
    RunState::Completed,
    RunState::Failed,
    RunState::Cancelled,
];

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    coordinator: CommandCoordinator,
    ids: Arc<DeterministicIds>,
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
        let clock = Arc::new(TestClock::new(SEED_MS));
        let mut registry = CommandRegistry::new();
        runtime::register_handlers(
            &mut registry,
            RuntimeDeps {
                clock: clock.clone(),
                ids: ids.clone(),
            },
        )
        .expect("runtime handlers register");
        let coordinator = CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            clock.clone(),
            Arc::new(ArmedFaults::new()),
        );
        let principal = PrincipalId::new(ids.as_ref());
        let actor = ActorId::new(ids.as_ref());
        Self {
            _dir: dir,
            store,
            coordinator,
            ids,
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

    async fn seed_task(&self, task_id: TaskId) {
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: self.actor,
                task_kind: "claim".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.graph().ensure_head(task_id).await.expect("seed head");
        txn.commit().await.expect("seed task commits");
    }

    /// Test-only scaffolding: production reaches `Ready` only through `BindRun`
    /// (config module), which this task does not own. Seeding the state through
    /// the repository lets claim eligibility be tested in isolation (D4).
    async fn seed_run(&self, task_id: TaskId, state: RunState) -> RunId {
        let run_id = RunId::new(self.ids.as_ref());
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("canonical stream key"),
            0,
        );
        let mut txn = self.write().await;
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id,
                session_id: None,
                parent_run_id: None,
                state,
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

    async fn seed_recovery(&self, run_id: RunId, recovery: RecoveryDisposition) {
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
                    recovery: Some(recovery),
                    ..RunPatch::default()
                },
            )
            .await
            .expect("recovery patch applies");
        assert!(updated, "recovery patch lands");
        txn.commit().await.expect("recovery patch commits");
    }

    async fn seed_claim(&self, run_id: RunId, owner: &str, token: u64, expires_unix_ms: i64) {
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
                    state: Some(RunState::Ready),
                    cancellation_epoch: None,
                },
                RunPatch {
                    claim: Some(ClaimPatch {
                        owner: owner.to_owned(),
                        token,
                        expires_unix_ms,
                        daemon_epoch: self.epoch,
                    }),
                    ..RunPatch::default()
                },
            )
            .await
            .expect("claim patch applies");
        assert!(updated, "claim patch lands");
        txn.commit().await.expect("claim patch commits");
    }

    async fn add(
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
        txn.commit().await.expect("edge commits");
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

    async fn record(
        &self,
        envelope: &CommandEnvelope,
    ) -> Option<kernel_store::models::IdempotencyRecordRow> {
        let mut txn = self.write().await;
        let row = txn
            .idempotency()
            .lookup(envelope.principal_id, &envelope.idempotency_key)
            .await
            .expect("idempotency read");
        txn.rollback().await.expect("rollback");
        row
    }

    fn envelope(&self, command_type: &str, key: &str, payload: Vec<u8>) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(key).expect("valid idempotency key"),
            principal_id: self.principal,
            actor_id: self.actor,
            device_id: None,
            delegation_chain_id: None,
            request_digest: RequestDigest::from_str(&"ab".repeat(32)).expect("valid digest"),
            correlation_id: Some("corr-claim".to_owned()),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: command_type.to_owned(),
            payload,
        }
    }

    /// Runs `claim` in its own transaction and commits regardless of outcome,
    /// so a mutation smuggled alongside an error would be observed by the next
    /// read (same policy as the state-machine suite).
    async fn try_claim(
        &self,
        run_id: RunId,
        owner: &str,
        ttl_ms: u64,
        now_ms: i64,
    ) -> errors::Result<()> {
        let mut txn = self.write().await;
        let result = claim(
            txn.as_mut(),
            run_id,
            owner.to_owned(),
            ttl_ms,
            now_ms,
            self.epoch,
        )
        .await;
        txn.commit()
            .await
            .expect("claim transaction commits regardless of outcome");
        result
    }
}

fn claim_payload(run_id: RunId, owner: &str, ttl_ms: u64) -> Vec<u8> {
    contract::ClaimReadyRun {
        run_id: run_id.to_string(),
        claim_owner: owner.to_owned(),
        claim_ttl_ms: ttl_ms,
    }
    .encode_to_vec()
}

fn claimed_events(events: &[OutboxEventRow]) -> Vec<&OutboxEventRow> {
    events
        .iter()
        .filter(|event| event.event_type == "RunClaimed" || event.event_type == "RunStarted")
        .collect()
}

#[tokio::test]
async fn claim_persists_fence_columns_and_stages_claimed_and_started() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;

    harness
        .try_claim(run, "worker-1", 30_000, SEED_MS)
        .await
        .expect("a ready run is claimed");

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(
        persisted.run_revision, 1,
        "the claim bumps the revision once"
    );
    assert_eq!(persisted.claim_owner.as_deref(), Some("worker-1"));
    assert!(
        persisted.claim_token.is_some_and(|token| token != 0),
        "the claim persists a token"
    );
    assert_eq!(persisted.claim_expires_ms, Some(SEED_MS + 30_000));
    assert_eq!(persisted.claim_daemon_epoch, Some(harness.epoch));
    assert_eq!(persisted.recovery, RecoveryDisposition::Normal);
    assert_eq!(persisted.terminal_reason, None);

    let events = harness.events().await;
    let staged = claimed_events(&events);
    assert_eq!(staged.len(), 2, "one RunClaimed and one RunStarted");
    assert_eq!(staged[0].event_type, "RunClaimed");
    assert_eq!(staged[1].event_type, "RunStarted");
    for (index, event) in staged.iter().enumerate() {
        assert_eq!(event.stream_key.as_str(), format!("run/{run}"));
        assert_eq!(event.sequence, index as u64 + 1, "contiguous run stream");
        assert_eq!(event.event_version, 1);
        assert_eq!(event.sensitivity, SensitivityClass::Internal);
        assert_eq!(event.retention, RetentionClass::Standard);
        let payload =
            contract::AgentRun::decode(event.payload.as_slice()).expect("payload decodes");
        assert_eq!(payload.run_id, run.to_string());
        assert_eq!(payload.state, RunState::Running.to_wire());
        assert_eq!(payload.run_revision, 1);
    }
}

#[tokio::test]
async fn claim_rejects_unmet_dependencies_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::WaitingTool).await;
    let target = harness.seed_run(task, RunState::Ready).await;
    harness
        .add(
            source,
            target,
            DependencyCondition::CompletedSuccessfully,
            0,
        )
        .await;

    let event_count = harness.events().await.len();
    let error = harness
        .try_claim(target, "worker-1", 30_000, SEED_MS)
        .await
        .expect_err("an unmet dependency blocks the claim");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let persisted = harness.run(target).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 0);
    assert_eq!(persisted.claim_owner, None);
    assert_eq!(harness.events().await.len(), event_count);
}

#[tokio::test]
async fn claim_accepts_satisfied_dependencies() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let cases = [
        (
            DependencyCondition::CompletedSuccessfully,
            RunState::Completed,
        ),
        (DependencyCondition::AnyTerminal, RunState::Failed),
        (
            DependencyCondition::CompletedOrCancelled,
            RunState::Cancelled,
        ),
    ];
    for (index, (condition, source_state)) in cases.into_iter().enumerate() {
        let source = harness.seed_run(task, source_state).await;
        let target = harness.seed_run(task, RunState::Ready).await;
        harness.add(source, target, condition, index as u64).await;
        harness
            .try_claim(target, "worker-1", 30_000, SEED_MS)
            .await
            .unwrap_or_else(|error| panic!("case {index}: {condition:?}: {error}"));
        let persisted = harness.run(target).await.expect("run persists");
        assert_eq!(persisted.state, RunState::Running, "case {index}");
    }
}

#[tokio::test]
async fn claim_rejects_non_normal_recovery_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let blocked = [
        RecoveryDisposition::NeedsReconciliation,
        RecoveryDisposition::Recovering,
        RecoveryDisposition::BlockedUnknownEffect,
        RecoveryDisposition::BlockedMissingResource,
        RecoveryDisposition::RequiresHumanDecision,
    ];
    for disposition in blocked {
        let run = harness.seed_run(task, RunState::Ready).await;
        harness.seed_recovery(run, disposition).await;

        let error = match harness.try_claim(run, "worker-1", 30_000, SEED_MS).await {
            Ok(()) => panic!("{disposition:?} admitted a claim"),
            Err(error) => error,
        };
        assert_eq!(
            error.code(),
            ErrorCode::FailedPrecondition,
            "{disposition:?}"
        );
        assert_eq!(error.retry_class(), RetryClass::Never, "{disposition:?}");

        let persisted = harness.run(run).await.expect("run persists");
        assert_eq!(persisted.state, RunState::Ready, "{disposition:?}");
        assert_eq!(persisted.run_revision, 0, "{disposition:?}");
        assert_eq!(persisted.recovery, disposition, "{disposition:?}");
        assert_eq!(persisted.claim_owner, None, "{disposition:?}");
    }
    assert!(
        harness.events().await.is_empty(),
        "rejections stage nothing"
    );
}

#[tokio::test]
async fn claim_rejects_a_live_claim_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;
    harness.seed_claim(run, "holder", 7, SEED_MS + 1).await;

    let error = harness
        .try_claim(run, "usurper", 30_000, SEED_MS)
        .await
        .expect_err("a live unexpired claim blocks the claim");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 0);
    assert_eq!(persisted.claim_owner.as_deref(), Some("holder"));
    assert_eq!(persisted.claim_token, Some(7));
    assert_eq!(persisted.claim_expires_ms, Some(SEED_MS + 1));
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn expired_claims_are_reclaimed_only_under_normal() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let reclaimable = harness.seed_run(task, RunState::Ready).await;
    harness.seed_claim(reclaimable, "stale", 7, SEED_MS).await;
    harness
        .try_claim(reclaimable, "fresh", 5_000, SEED_MS)
        .await
        .expect("an expired claim is reclaimable under Normal");
    let persisted = harness.run(reclaimable).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.run_revision, 1);
    assert_eq!(persisted.claim_owner.as_deref(), Some("fresh"));
    assert!(
        persisted.claim_token != Some(7),
        "reclamation mints a fresh token"
    );
    assert_eq!(persisted.claim_expires_ms, Some(SEED_MS + 5_000));
    assert_eq!(claimed_events(&harness.events().await).len(), 2);

    let blocked = harness.seed_run(task, RunState::Ready).await;
    harness.seed_claim(blocked, "stale", 9, SEED_MS - 500).await;
    harness
        .seed_recovery(blocked, RecoveryDisposition::NeedsReconciliation)
        .await;
    let error = harness
        .try_claim(blocked, "fresh", 5_000, SEED_MS)
        .await
        .expect_err("a non-Normal disposition blocks reclamation");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    let persisted = harness.run(blocked).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 0);
    assert_eq!(persisted.claim_owner.as_deref(), Some("stale"));
    assert_eq!(persisted.claim_token, Some(9));
    assert_eq!(
        claimed_events(&harness.events().await).len(),
        2,
        "only the reclaimable run staged events"
    );
}

#[tokio::test]
async fn claim_rejects_every_non_ready_state_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    for state in PERSISTED_STATES {
        if state == RunState::Ready {
            continue;
        }
        let run = harness.seed_run(task, state).await;
        let error = match harness.try_claim(run, "worker-1", 30_000, SEED_MS).await {
            Ok(()) => panic!("{state:?} admitted a claim"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::Conflict, "{state:?}");
        assert_eq!(error.retry_class(), RetryClass::Never, "{state:?}");

        let persisted = harness.run(run).await.expect("run persists");
        assert_eq!(persisted.state, state, "{state:?}");
        assert_eq!(persisted.run_revision, 0, "{state:?}");
        assert_eq!(persisted.claim_owner, None, "{state:?}");
    }
    assert!(
        harness.events().await.is_empty(),
        "rejections stage nothing"
    );
}

#[tokio::test]
async fn missing_runs_are_not_found() {
    let harness = Harness::new().await;
    let absent = RunId::new(harness.ids.as_ref());
    let error = harness
        .try_claim(absent, "worker-1", 30_000, SEED_MS)
        .await
        .expect_err("unknown runs cannot be claimed");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn hundred_claimants_yield_exactly_one_winner() {
    let harness = Arc::new(Harness::new().await);
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;

    let barrier = Arc::new(Barrier::new(100));
    let mut claimants = Vec::new();
    for index in 0..100 {
        let harness = Arc::clone(&harness);
        let barrier = Arc::clone(&barrier);
        claimants.push(tokio::spawn(async move {
            let owner = format!("racer-{index}");
            barrier.wait().await;
            let result = harness.try_claim(run, &owner, 60_000, SEED_MS).await;
            (owner, result)
        }));
    }

    let mut winners = Vec::new();
    let mut losers = 0_usize;
    for claimant in claimants {
        let (owner, result) = claimant.await.expect("claimant joins");
        match result {
            Ok(()) => winners.push(owner),
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::Conflict, "{owner}");
                assert_eq!(error.retry_class(), RetryClass::Never, "{owner}");
                losers += 1;
            }
        }
    }
    assert_eq!(winners.len(), 1, "exactly one claimant commits");
    assert_eq!(losers, 99);

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.run_revision, 1);
    assert_eq!(persisted.claim_owner.as_deref(), Some(winners[0].as_str()));
    assert_eq!(persisted.claim_expires_ms, Some(SEED_MS + 60_000));

    let events = harness.events().await;
    assert_eq!(events.len(), 2, "the winner stages exactly two events");
    assert_eq!(events[0].event_type, "RunClaimed");
    assert_eq!(events[1].event_type, "RunStarted");
}

#[tokio::test]
async fn claim_handler_claims_a_ready_run_through_the_coordinator() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;
    let envelope = harness.envelope(
        CMD_CLAIM_READY_RUN,
        "claim-1",
        claim_payload(run, "daemon-a", 15_000),
    );

    let outcome = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect("claim succeeds through the coordinator");
    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(outcome.payload, run.to_string().into_bytes());

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.run_revision, 1);
    assert_eq!(persisted.claim_owner.as_deref(), Some("daemon-a"));
    assert_eq!(persisted.claim_expires_ms, Some(SEED_MS + 15_000));
    assert_eq!(persisted.claim_daemon_epoch, Some(harness.epoch));
    assert_eq!(claimed_events(&harness.events().await).len(), 2);
    assert!(harness.record(&envelope).await.is_some());

    let replay = harness
        .coordinator
        .execute(envelope)
        .await
        .expect("identical replay returns the stored outcome");
    assert_eq!(replay, outcome);
    assert_eq!(claimed_events(&harness.events().await).len(), 2);
}

#[tokio::test]
async fn claim_handler_rejects_malformed_payloads_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;

    let envelope = harness.envelope(
        CMD_CLAIM_READY_RUN,
        "claim-malformed",
        b"do-not-echo-claimable-payload".to_vec(),
    );
    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect_err("malformed payloads are invalid arguments");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(!error.message().contains("do-not-echo"));

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 0);
    assert!(harness.events().await.is_empty());
    assert!(harness.record(&envelope).await.is_none());
}

#[tokio::test]
async fn claim_handler_requires_a_run_and_an_owner() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Ready).await;

    let absent_run = harness.envelope(
        CMD_CLAIM_READY_RUN,
        "claim-no-run",
        claim_payload(RunId::new(harness.ids.as_ref()), "", 30_000),
    );
    let absent_owner = harness.envelope(
        CMD_CLAIM_READY_RUN,
        "claim-no-owner",
        claim_payload(run, "", 30_000),
    );

    for envelope in [absent_run, absent_owner] {
        let error = harness
            .coordinator
            .execute(envelope)
            .await
            .expect_err("missing claim fields are invalid arguments");
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert_eq!(error.retry_class(), RetryClass::Never);
    }

    let persisted = harness.run(run).await.expect("run persists");
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 0);
    assert!(harness.events().await.is_empty());
}

#[test]
fn register_handlers_registers_claim_ready_run() {
    let mut registry = CommandRegistry::new();
    runtime::register_handlers(
        &mut registry,
        RuntimeDeps {
            clock: Arc::new(TestClock::new(SEED_MS)),
            ids: Arc::new(DeterministicIds::new(SEED_MS)),
        },
    )
    .expect("runtime handlers register");
    assert_eq!(registry.len(), 9);
    assert!(registry.get(CMD_CLAIM_READY_RUN).is_some());
    assert!(registry.get(runtime::CMD_BIND_RUN).is_some());
    assert!(registry.get(runtime::CMD_RESOLVE_UNKNOWN_EFFECT).is_some());
    assert!(registry.get(runtime::CMD_SCHEDULE_TIMER).is_some());
    assert!(registry.get(runtime::CMD_CANCEL_TIMER).is_some());
}
