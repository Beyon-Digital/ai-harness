//! Cancellation acceptance tests (R5.1-R5.5, P3): epoch fencing, subtree
//! propagation through the command coordinator, duplicate cancellation, and
//! the barrier-controlled cancel/spawn race over the real SQLite store.

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
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewTask, OutboxEventRow, RunRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use runtime::cancel::cancel;
use runtime::{CMD_CANCEL_RUN, CMD_CREATE_TASK_RUN, RuntimeDeps};
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";

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
                task_kind: "cancel".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.graph().ensure_head(task_id).await.expect("seed head");
        txn.commit().await.expect("seed task commits");
    }

    async fn seed_run(&self, task: TaskId, parent: Option<RunId>, state: RunState) -> RunId {
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
                parent_run_id: parent,
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

    async fn run(&self, run_id: RunId) -> Option<RunRow> {
        let mut txn = self.write().await;
        let row = txn.runs().get(run_id).await.expect("run read");
        txn.rollback().await.expect("rollback");
        row
    }

    /// Bumps a run's revision through a no-op patch so a nonzero revision
    /// expectation can be exercised.
    async fn bump_revision(&self, run_id: RunId) {
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
                kernel_store::models::RunCas {
                    run_revision: current.run_revision,
                    state: None,
                    cancellation_epoch: None,
                },
                kernel_store::models::RunPatch {
                    bump_revision: true,
                    ..kernel_store::models::RunPatch::default()
                },
            )
            .await
            .expect("revision patch applies");
        assert!(updated, "revision patch lands");
        txn.commit().await.expect("revision patch commits");
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

    fn envelope(&self, command_type: &str, key: &str, payload: Vec<u8>) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(key).expect("valid idempotency key"),
            principal_id: self.principal,
            actor_id: self.actor,
            device_id: None,
            delegation_chain_id: None,
            request_digest: RequestDigest::from_str(&"ab".repeat(32)).expect("valid digest"),
            correlation_id: Some("corr-cancel".to_owned()),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: command_type.to_owned(),
            payload,
        }
    }
}

fn cancel_payload(run_id: RunId, expected_run_revision: u64, reason: &str) -> Vec<u8> {
    contract::CancelRun {
        run_id: run_id.to_string(),
        expected_run_revision,
        reason: reason.to_owned(),
    }
    .encode_to_vec()
}

fn create_payload(
    task_id: TaskId,
    run_id: RunId,
    parent: RunId,
    observed_parent_cancellation_epoch: u64,
) -> Vec<u8> {
    contract::CreateTaskRun {
        task_id: task_id.to_string(),
        run_id: run_id.to_string(),
        session_id: String::new(),
        task_kind: "cancel".to_owned(),
        task_payload: Vec::new(),
        agent_spec_ref: None,
        parent_run_id: parent.to_string(),
        observed_parent_cancellation_epoch,
        requested_profile: String::new(),
        workspace_uri: String::new(),
        requested_capabilities: Vec::new(),
        requested_budget: Vec::new(),
    }
    .encode_to_vec()
}

fn events_of<'a>(events: &'a [OutboxEventRow], event_type: &str) -> Vec<&'a OutboxEventRow> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

fn stream_events_for(events: &[OutboxEventRow], run_id: RunId) -> Vec<&OutboxEventRow> {
    events
        .iter()
        .filter(|event| event.stream_key.as_str() == format!("run/{run_id}"))
        .collect()
}

#[tokio::test]
async fn cancel_transitions_the_root_and_nested_descendants() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    let created = harness.seed_run(task, Some(root), RunState::Created).await;
    let ready = harness.seed_run(task, Some(root), RunState::Ready).await;
    let grandchild = harness
        .seed_run(task, Some(created), RunState::Running)
        .await;
    let terminal = harness
        .seed_run(task, Some(root), RunState::Completed)
        .await;
    let cancelling = harness
        .seed_run(task, Some(root), RunState::Cancelling)
        .await;
    let unrelated = harness.seed_run(task, None, RunState::Running).await;

    let envelope = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-nested",
        cancel_payload(root, 0, "operator cancelled"),
    );
    let outcome = harness
        .coordinator
        .execute(envelope)
        .await
        .expect("the cancel commits");
    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(outcome.payload, root.to_string().into_bytes());

    let root_row = harness.run(root).await.expect("root persists");
    assert_eq!(root_row.state, RunState::Cancelling);
    assert_eq!(root_row.cancellation_epoch, 1);
    assert_eq!(root_row.run_revision, 2, "epoch bump plus one transition");
    assert_eq!(root_row.terminal_reason, None);

    let created_row = harness.run(created).await.expect("created child persists");
    assert_eq!(
        created_row.state,
        RunState::Cancelled,
        "a never-started run"
    );
    assert_eq!(
        created_row.cancellation_epoch, 0,
        "only the root's epoch moves"
    );
    assert_eq!(
        created_row.run_revision, 1,
        "the cancellation-only edge commits once"
    );
    assert_eq!(
        created_row.terminal_reason.as_deref(),
        Some("operator cancelled")
    );

    let ready_row = harness.run(ready).await.expect("ready child persists");
    assert_eq!(ready_row.state, RunState::Cancelled);
    assert_eq!(ready_row.run_revision, 1);
    assert_eq!(
        ready_row.terminal_reason.as_deref(),
        Some("operator cancelled")
    );

    let grandchild_row = harness.run(grandchild).await.expect("grandchild persists");
    assert_eq!(grandchild_row.state, RunState::Cancelling);
    assert_eq!(grandchild_row.run_revision, 1);
    assert_eq!(grandchild_row.terminal_reason, None);

    let terminal_row = harness
        .run(terminal)
        .await
        .expect("terminal child persists");
    assert_eq!(terminal_row.state, RunState::Completed);
    assert_eq!(terminal_row.run_revision, 0);
    assert_eq!(terminal_row.terminal_reason, None);

    let cancelling_row = harness
        .run(cancelling)
        .await
        .expect("cancelling child persists");
    assert_eq!(cancelling_row.state, RunState::Cancelling);
    assert_eq!(cancelling_row.run_revision, 0);

    let unrelated_row = harness.run(unrelated).await.expect("unrelated persists");
    assert_eq!(unrelated_row.state, RunState::Running);
    assert_eq!(unrelated_row.cancellation_epoch, 0);

    let events = harness.events().await;
    assert_eq!(events_of(&events, "CancellationEpochAdvanced").len(), 1);
    assert_eq!(events_of(&events, "RunCancelled").len(), 2);
    assert_eq!(events_of(&events, "RunStateChanged").len(), 2);

    let root_events = stream_events_for(&events, root);
    assert_eq!(root_events.len(), 2, "epoch then state on the root stream");
    assert_eq!(root_events[0].event_type, "CancellationEpochAdvanced");
    assert_eq!(root_events[0].sequence, 1);
    assert_eq!(root_events[0].retention, RetentionClass::Audit);
    let payload =
        contract::AgentRun::decode(root_events[0].payload.as_slice()).expect("payload decodes");
    assert_eq!(payload.cancellation_epoch, 1);
    assert_eq!(payload.run_revision, 1);
    assert_eq!(root_events[1].event_type, "RunStateChanged");
    assert_eq!(root_events[1].sequence, 2);
    assert_eq!(root_events[1].retention, RetentionClass::Standard);
    assert_eq!(root_events[1].sensitivity, SensitivityClass::Internal);

    for (run_id, expected_state) in [
        (created, RunState::Cancelled),
        (ready, RunState::Cancelled),
        (grandchild, RunState::Cancelling),
    ] {
        let run_events = stream_events_for(&events, run_id);
        assert_eq!(run_events.len(), 1, "exactly one event for {run_id}");
        assert_eq!(run_events[0].sequence, 1);
        let payload =
            contract::AgentRun::decode(run_events[0].payload.as_slice()).expect("payload decodes");
        assert_eq!(payload.state, expected_state.to_wire());
        assert_eq!(payload.run_id, run_id.to_string());
    }

    for run_id in [terminal, cancelling, unrelated] {
        assert!(
            stream_events_for(&events, run_id).is_empty(),
            "unchanged runs stage nothing"
        );
    }
}

#[tokio::test]
async fn cancel_service_reports_the_changed_runs() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    let child = harness.seed_run(task, Some(root), RunState::Created).await;

    let mut txn = harness.write().await;
    let report = cancel(txn.as_mut(), root, None, "operator cancelled", SEED_MS)
        .await
        .expect("the cancel service commits");
    txn.commit()
        .await
        .expect("cancel service transaction commits");

    assert_eq!(report.root, root);
    assert_eq!(report.cancellation_epoch, 1);
    assert_eq!(report.changed, vec![root, child]);
}

#[tokio::test]
async fn duplicate_cancel_advances_the_epoch_and_leaves_terminals_untouched() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    let child = harness.seed_run(task, Some(root), RunState::Created).await;

    let first = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-duplicate-1",
        cancel_payload(root, 0, "operator cancelled"),
    );
    harness
        .coordinator
        .execute(first)
        .await
        .expect("the first cancel commits");

    let after_first = harness.run(root).await.expect("root persists");
    assert_eq!(after_first.run_revision, 2);

    let second = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-duplicate-2",
        cancel_payload(root, after_first.run_revision, "operator cancelled"),
    );
    harness
        .coordinator
        .execute(second)
        .await
        .expect("a duplicate cancel commits");

    let root_row = harness.run(root).await.expect("root persists");
    assert_eq!(root_row.state, RunState::Cancelling);
    assert_eq!(root_row.cancellation_epoch, 2, "the epoch advances again");
    assert_eq!(root_row.run_revision, 3, "only the epoch CAS moves");

    let child_row = harness.run(child).await.expect("child persists");
    assert_eq!(child_row.state, RunState::Cancelled);
    assert_eq!(child_row.run_revision, 1, "terminal runs are untouched");

    let events = harness.events().await;
    assert_eq!(events_of(&events, "CancellationEpochAdvanced").len(), 2);
    assert_eq!(events_of(&events, "RunCancelled").len(), 1);
    assert_eq!(events_of(&events, "RunStateChanged").len(), 1);
}

#[tokio::test]
async fn cancel_rejects_a_stale_expected_revision_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;

    let envelope = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-stale",
        cancel_payload(root, 7, "operator cancelled"),
    );
    let error = harness
        .coordinator
        .execute(envelope)
        .await
        .expect_err("a stale expected revision rejects the cancel");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.cancellation_epoch, 0);
    assert_eq!(persisted.run_revision, 0);
    assert!(harness.events().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cancels_sharing_one_expectation_commit_exactly_once() {
    let harness = Arc::new(Harness::new().await);
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    harness.bump_revision(root).await;
    let expected = harness.run(root).await.expect("root persists").run_revision;
    assert_eq!(expected, 1, "the shared expectation is nonzero");

    let barrier = Arc::new(Barrier::new(2));
    let mut cancels = Vec::new();
    for _ in 0..2 {
        let harness = Arc::clone(&harness);
        let barrier = Arc::clone(&barrier);
        cancels.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut txn = harness.write().await;
            let result = cancel(txn.as_mut(), root, Some(expected), "race", SEED_MS).await;
            txn.commit()
                .await
                .expect("cancel transaction commits regardless of outcome");
            result
        }));
    }

    let mut winners = 0_usize;
    let mut losers = 0_usize;
    for cancel_task in cancels {
        match cancel_task.await.expect("cancel task joins") {
            Ok(_) => winners += 1,
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::Conflict);
                assert_eq!(error.retry_class(), RetryClass::Never);
                losers += 1;
            }
        }
    }
    assert_eq!(winners, 1, "exactly one cancel commits");
    assert_eq!(losers, 1, "the stale expectation loses");

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.state, RunState::Cancelling);
    assert_eq!(persisted.cancellation_epoch, 1, "exactly one epoch advance");
    assert_eq!(
        persisted.run_revision, 3,
        "one seeded bump, one epoch bump, one transition"
    );

    let events = harness.events().await;
    assert_eq!(
        events_of(&events, "CancellationEpochAdvanced").len(),
        1,
        "the loser stages no epoch event"
    );
    assert_eq!(events_of(&events, "RunStateChanged").len(), 1);
    assert_eq!(events_of(&events, "RunCancelled").len(), 0);
}

#[tokio::test]
async fn cancel_rejects_unknown_runs_and_empty_reasons() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;

    let absent = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-absent",
        cancel_payload(RunId::new(harness.ids.as_ref()), 0, "operator cancelled"),
    );
    let error = harness
        .coordinator
        .execute(absent)
        .await
        .expect_err("unknown runs are not found");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let blank = harness.envelope(
        CMD_CANCEL_RUN,
        "cancel-blank",
        cancel_payload(root, 0, "   "),
    );
    let error = harness
        .coordinator
        .execute(blank)
        .await
        .expect_err("a blank reason is invalid");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.run_revision, 0);
    assert!(harness.events().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_wins_the_spawn_race_and_no_child_row_escapes() {
    let harness = Arc::new(Harness::new().await);
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let parent = harness.seed_run(task, None, RunState::Running).await;
    let child = RunId::new(harness.ids.as_ref());

    let held = Arc::new(Barrier::new(2));
    let released = Arc::new(Barrier::new(2));

    let spawn = tokio::spawn({
        let harness = Arc::clone(&harness);
        let held = Arc::clone(&held);
        let released = Arc::clone(&released);
        async move {
            let observed = harness
                .run(parent)
                .await
                .expect("parent persists")
                .cancellation_epoch;
            held.wait().await;
            released.wait().await;
            let envelope = harness.envelope(
                CMD_CREATE_TASK_RUN,
                "race-spawn",
                create_payload(task, child, parent, observed),
            );
            harness.coordinator.execute(envelope).await
        }
    });

    held.wait().await;
    let cancelled = harness
        .coordinator
        .execute(harness.envelope(
            CMD_CANCEL_RUN,
            "race-cancel",
            cancel_payload(parent, 0, "race"),
        ))
        .await
        .expect("the cancel commits while the spawn is held");
    assert_eq!(cancelled.code, OutcomeCode::Ok);
    released.wait().await;

    let error = spawn
        .await
        .expect("the spawn task joins")
        .expect_err("a spawn holding the pre-cancel epoch must be rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);

    assert!(
        harness.run(child).await.is_none(),
        "no child row may exist after the race"
    );
    let events = harness.events().await;
    assert!(
        stream_events_for(&events, child).is_empty(),
        "no child event may exist after the race"
    );

    let parent_row = harness.run(parent).await.expect("parent persists");
    assert_eq!(parent_row.state, RunState::Cancelling);
    assert_eq!(parent_row.cancellation_epoch, 1);
}

#[test]
fn register_handlers_registers_cancel_run() {
    let mut registry = CommandRegistry::new();
    runtime::register_handlers(
        &mut registry,
        RuntimeDeps {
            clock: Arc::new(TestClock::new(SEED_MS)),
            ids: Arc::new(DeterministicIds::new(SEED_MS)),
        },
    )
    .expect("runtime handlers register");
    assert!(registry.get(CMD_CANCEL_RUN).is_some());
}
