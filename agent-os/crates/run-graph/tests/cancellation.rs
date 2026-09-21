//! Cancellation acceptance tests (R5.1, R5.3-R5.5): the epoch advance, the
//! known-subtree walk over `runs.parent_run_id`, and the eligibility rules
//! that tell the caller which runs still need a state transition.

use std::sync::Arc;

use domain::generated::contract;
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewTask, OutboxEventRow, RunCas, RunPatch, RunRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use run_graph::cancellation::cancel_subtree;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: Arc<DeterministicIds>,
    principal: PrincipalId,
    epoch: u64,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp root");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join(DB_FILE),
                pool_max_connections: 5,
                busy_timeout_ms: 5_000,
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
            principal,
            epoch: fence.epoch.0,
        }
    }

    async fn write(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(TxContext {
                daemon_epoch: self.epoch,
                principal_id: self.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await
            .expect("write transaction opens")
    }

    async fn seed_task(&self, task_id: TaskId) {
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: ActorId::new(self.ids.as_ref()),
                task_kind: "cancellation".to_owned(),
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

    async fn set_state(&self, run_id: RunId, state: RunState) {
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

    /// Runs `cancel_subtree` in its own transaction and commits regardless of
    /// outcome, so a mutation smuggled alongside an error would be observed by
    /// the next read (same policy as the state-machine suite).
    async fn try_cancel(
        &self,
        root: RunId,
        expected_run_revision: Option<u64>,
        reason: &str,
    ) -> errors::Result<Vec<RunId>> {
        let mut txn = self.write().await;
        let result =
            cancel_subtree(txn.as_mut(), root, expected_run_revision, reason, SEED_MS).await;
        txn.commit()
            .await
            .expect("cancel transaction commits regardless of outcome");
        result
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
                RunCas {
                    run_revision: current.run_revision,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await
            .expect("revision patch applies");
        assert!(updated, "revision patch lands");
        txn.commit().await.expect("revision patch commits");
    }
}

fn event_streams(events: &[OutboxEventRow], event_type: &str) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .map(|event| event.stream_key.as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn cancel_subtree_advances_the_root_epoch_and_stages_the_epoch_event() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;

    let eligible = harness
        .try_cancel(root, None, "operator cancelled")
        .await
        .expect("cancel commits");
    assert_eq!(eligible, vec![root], "a running root is eligible");

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.cancellation_epoch, 1);
    assert_eq!(
        persisted.run_revision, 1,
        "the epoch advance bumps the revision exactly once"
    );
    assert_eq!(
        persisted.state,
        RunState::Running,
        "the graph service stages the fence and reports eligibility only"
    );

    let events = harness.events().await;
    assert_eq!(events.len(), 1, "only the epoch event is staged here");
    let event = &events[0];
    assert_eq!(event.event_type, "CancellationEpochAdvanced");
    assert_eq!(event.stream_key.as_str(), format!("run/{root}"));
    assert_eq!(event.sequence, 1);
    assert_eq!(event.event_version, 1);
    assert_eq!(event.sensitivity, SensitivityClass::Internal);
    assert_eq!(event.retention, RetentionClass::Audit);
    let payload = contract::AgentRun::decode(event.payload.as_slice()).expect("payload decodes");
    assert_eq!(payload.run_id, root.to_string());
    assert_eq!(payload.cancellation_epoch, 1);
    assert_eq!(payload.run_revision, 1);
    assert_eq!(payload.state, RunState::Running.to_wire());
}

#[tokio::test]
async fn cancel_subtree_walks_nested_descendants_in_breadth_first_order() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    let child_a = harness.seed_run(task, Some(root), RunState::Created).await;
    let child_b = harness
        .seed_run(task, Some(root), RunState::Suspended)
        .await;
    let grandchild_a = harness
        .seed_run(task, Some(child_a), RunState::Running)
        .await;
    let grandchild_b = harness
        .seed_run(task, Some(child_a), RunState::WaitingTool)
        .await;
    let unrelated = harness.seed_run(task, None, RunState::Running).await;

    let eligible = harness
        .try_cancel(root, None, "operator cancelled")
        .await
        .expect("cancel commits");
    assert_eq!(
        eligible,
        vec![root, child_a, child_b, grandchild_a, grandchild_b],
        "the root comes first, then descendants breadth-first"
    );
    assert!(
        !eligible.contains(&unrelated),
        "siblings are not descendants"
    );
}

#[tokio::test]
async fn cancel_subtree_lists_only_eligible_runs() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;

    let children = [
        (RunState::Created, true),
        (RunState::Ready, true),
        (RunState::Running, true),
        (RunState::WaitingTool, true),
        (RunState::WaitingChild, true),
        (RunState::WaitingHuman, true),
        (RunState::Suspended, true),
        (RunState::Cancelling, false),
        (RunState::Completed, false),
        (RunState::Failed, false),
        (RunState::Cancelled, false),
    ];
    let mut expected = vec![root];
    let mut skipped = Vec::new();
    for (state, eligible) in children {
        let child = harness.seed_run(task, Some(root), state).await;
        if eligible {
            expected.push(child);
        } else {
            skipped.push(child);
        }
    }

    let eligible = harness
        .try_cancel(root, None, "operator cancelled")
        .await
        .expect("cancel commits");
    assert_eq!(eligible, expected, "only non-terminal, non-cancelling runs");

    for run_id in &skipped {
        let persisted = harness.run(*run_id).await.expect("skipped run persists");
        assert_eq!(persisted.run_revision, 0, "skipped runs are untouched");
        assert_eq!(
            persisted.cancellation_epoch, 0,
            "only the root's epoch moves"
        );
    }

    let events = harness.events().await;
    assert_eq!(
        event_streams(&events, "CancellationEpochAdvanced"),
        vec![format!("run/{root}")],
        "exactly one epoch event, on the root stream"
    );
    for run_id in &skipped {
        assert!(
            !events
                .iter()
                .any(|event| event.stream_key.as_str() == format!("run/{run_id}")),
            "skipped runs stage nothing"
        );
    }
}

#[tokio::test]
async fn duplicate_cancel_advances_the_epoch_again_and_lists_nothing() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;

    let first = harness
        .try_cancel(root, None, "operator cancelled")
        .await
        .expect("first cancel commits");
    assert_eq!(first, vec![root]);
    harness.set_state(root, RunState::Cancelling).await;

    let second = harness
        .try_cancel(root, None, "operator cancelled")
        .await
        .expect("a duplicate cancel commits");
    assert!(second.is_empty(), "an already-cancelling root is skipped");

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.cancellation_epoch, 2, "the epoch advances again");
    assert_eq!(persisted.run_revision, 2);
    assert_eq!(persisted.state, RunState::Cancelling);

    let events = harness.events().await;
    assert_eq!(
        event_streams(&events, "CancellationEpochAdvanced"),
        vec![format!("run/{root}"), format!("run/{root}")],
        "each cancel stages its own epoch event"
    );
}

#[tokio::test]
async fn stale_revision_expectations_conflict_without_an_epoch_advance() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, None, RunState::Running).await;
    harness.bump_revision(root).await;

    let error = harness
        .try_cancel(root, Some(0), "operator cancelled")
        .await
        .expect_err("a stale expectation conflicts");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.state, RunState::Running);
    assert_eq!(persisted.cancellation_epoch, 0, "no epoch advance");
    assert_eq!(persisted.run_revision, 1);
    assert!(harness.events().await.is_empty(), "no staged epoch event");

    let eligible = harness
        .try_cancel(root, Some(1), "operator cancelled")
        .await
        .expect("the matching expectation commits");
    assert_eq!(eligible, vec![root]);
    let persisted = harness.run(root).await.expect("root persists");
    assert_eq!(persisted.cancellation_epoch, 1);
    assert_eq!(persisted.run_revision, 2);
}

#[tokio::test]
async fn missing_roots_are_not_found() {
    let harness = Harness::new().await;
    let absent = RunId::new(harness.ids.as_ref());
    let error = harness
        .try_cancel(absent, None, "operator cancelled")
        .await
        .expect_err("unknown roots cannot be cancelled");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.events().await.is_empty());
}
