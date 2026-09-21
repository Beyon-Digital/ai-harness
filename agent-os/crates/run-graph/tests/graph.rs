//! Dependency edge acceptance tests over the real SQLite kernel store: same
//! task scope, mutable targets, transactional head plus `DependencyAdded`,
//! deterministic duplicates, committed acyclicity under races and arbitrary
//! interleavings, and parent-link ancestry without a second table (R3, P1, N2).

use std::sync::Arc;

use domain::generated::contract;
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
};
use domain::resource::DependencyCondition;
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{
    ClaimPatch, NewRun, NewTask, OutboxEventRow, RunCas, RunDependencyRow, RunGraphHeadRow,
    RunPatch,
};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use proptest::prelude::*;
use prost::Message;
use run_graph::graph::add_dependency;
use run_graph::repository::descendants;
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

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
                pool_max_connections: 8,
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
                created_by_actor_id: ActorId::new(self.ids.as_ref()),
                task_kind: "test".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.graph().ensure_head(task_id).await.expect("seed head");
        txn.commit().await.expect("seed task commits");
    }

    async fn seed_run(&self, task_id: TaskId, state: RunState) -> RunId {
        self.seed_linked_run(task_id, None, state).await
    }

    async fn seed_child(&self, task_id: TaskId, parent: RunId) -> RunId {
        self.seed_linked_run(task_id, Some(parent), RunState::Created)
            .await
    }

    async fn seed_linked_run(
        &self,
        task_id: TaskId,
        parent_run_id: Option<RunId>,
        state: RunState,
    ) -> RunId {
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
                parent_run_id,
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

    async fn seed_claim(&self, run_id: RunId, expires_unix_ms: i64) {
        let mut txn = self.write().await;
        let updated = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: 0,
                    state: Some(RunState::Ready),
                    cancellation_epoch: None,
                },
                RunPatch {
                    claim: Some(ClaimPatch {
                        owner: "worker-1".to_owned(),
                        token: 1,
                        expires_unix_ms,
                        daemon_epoch: self.epoch,
                    }),
                    ..RunPatch::default()
                },
            )
            .await
            .expect("claim patch applies");
        assert!(updated, "claim patch must land");
        txn.commit().await.expect("claim commits");
    }

    async fn add(
        &self,
        source: RunId,
        target: RunId,
        condition: DependencyCondition,
        expected_revision: u64,
    ) -> errors::Result<()> {
        let mut txn = self.write().await;
        match add_dependency(
            txn.as_mut(),
            source,
            target,
            condition,
            expected_revision,
            SEED_MS,
        )
        .await
        {
            Ok(()) => {
                txn.commit().await.expect("edge commits");
                Ok(())
            }
            Err(error) => {
                txn.rollback().await.expect("edge rolls back");
                Err(error)
            }
        }
    }

    async fn dependencies(&self, task_id: TaskId) -> Vec<RunDependencyRow> {
        let mut txn = self.write().await;
        let rows = txn
            .graph()
            .list_dependencies(task_id)
            .await
            .expect("dependencies read");
        txn.rollback().await.expect("rollback");
        rows
    }

    async fn head(&self, task_id: TaskId) -> Option<RunGraphHeadRow> {
        let mut txn = self.write().await;
        let row = txn.graph().get_head(task_id).await.expect("head read");
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

    async fn descendants(&self, root: RunId) -> errors::Result<Vec<RunId>> {
        let mut txn = self.write().await;
        let result = descendants(txn.as_mut(), root).await;
        txn.rollback().await.expect("rollback");
        result
    }

    async fn assert_acyclic(&self, runs: &[RunId]) {
        let mut read = self.store.begin_read().await.expect("read transaction");
        for left in runs {
            for right in runs {
                if left == right {
                    continue;
                }
                let forward = read
                    .graph()
                    .is_reachable(*left, *right)
                    .await
                    .expect("reachability read");
                let backward = read
                    .graph()
                    .is_reachable(*right, *left)
                    .await
                    .expect("reachability read");
                assert!(
                    !(forward && backward),
                    "committed graph contains a cycle through {left} and {right}"
                );
            }
        }
    }
}

#[tokio::test]
async fn edge_insertion_advances_head_and_stages_dependency_added() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    harness
        .add(
            source,
            target,
            DependencyCondition::CompletedSuccessfully,
            0,
        )
        .await
        .expect("edge commits");

    let head = harness.head(task).await.expect("head present");
    assert_eq!(
        head.graph_revision, 1,
        "one committed edge advances the head"
    );
    let dependencies = harness.dependencies(task).await;
    assert_eq!(dependencies.len(), 1);
    let edge = &dependencies[0];
    assert_eq!(edge.task_id, task);
    assert_eq!(edge.source_run_id, source);
    assert_eq!(edge.target_run_id, target);
    assert_eq!(
        edge.dependency_condition,
        DependencyCondition::CompletedSuccessfully
    );
    assert_eq!(edge.created_graph_revision, 0);
    assert_eq!(edge.created_at_ms, SEED_MS);

    let events = harness.events().await;
    assert_eq!(events.len(), 1, "one edge stages one event");
    let event = &events[0];
    assert_eq!(event.event_type, "DependencyAdded");
    assert_eq!(event.event_version, 1);
    assert_eq!(event.stream_key.as_str(), format!("task/{task}"));
    assert_eq!(event.sequence, 1);
    assert_eq!(event.sensitivity, SensitivityClass::Internal);
    assert_eq!(event.retention, RetentionClass::Standard);
    assert_eq!(event.task_id, None);
    let payload =
        contract::RunDependency::decode(event.payload.as_slice()).expect("payload decodes");
    assert_eq!(payload.dependency_id, edge.dependency_id.to_string());
    assert_eq!(payload.task_id, task.to_string());
    assert_eq!(payload.source_run_id, source.to_string());
    assert_eq!(payload.target_run_id, target.to_string());
    assert_eq!(
        payload.condition,
        DependencyCondition::CompletedSuccessfully.to_wire()
    );
    assert_eq!(payload.created_graph_revision, 0);
}

#[tokio::test]
async fn rollback_discards_the_edge_head_and_event() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    let mut txn = harness.write().await;
    add_dependency(
        txn.as_mut(),
        source,
        target,
        DependencyCondition::AnyTerminal,
        0,
        SEED_MS,
    )
    .await
    .expect("edge stages inside the transaction");
    txn.rollback().await.expect("rollback");

    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        0
    );
    assert!(harness.dependencies(task).await.is_empty());
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn self_edge_is_rejected_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let run = harness.seed_run(task, RunState::Created).await;

    let error = harness
        .add(run, run, DependencyCondition::AnyTerminal, 0)
        .await
        .expect_err("self edge is rejected");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.dependencies(task).await.is_empty());
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        0
    );
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn cross_task_edges_are_rejected_without_mutation() {
    let harness = Harness::new().await;
    let left_task = TaskId::new(harness.ids.as_ref());
    let right_task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(left_task).await;
    harness.seed_task(right_task).await;
    let left = harness.seed_run(left_task, RunState::Created).await;
    let right = harness.seed_run(right_task, RunState::Created).await;

    for (source, target, task) in [(left, right, left_task), (right, left, right_task)] {
        let error = harness
            .add(source, target, DependencyCondition::AnyTerminal, 0)
            .await
            .expect_err("cross-task edge is rejected");
        assert_eq!(error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(harness.dependencies(task).await.is_empty());
        assert_eq!(
            harness
                .head(task)
                .await
                .expect("head present")
                .graph_revision,
            0
        );
    }
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn target_must_be_created_or_unclaimed_ready() {
    let harness = Harness::new().await;
    let mutable = [
        (RunState::Created, None),
        (RunState::Ready, None),
        (RunState::Ready, Some(SEED_MS - 1)),
    ];
    for (index, (state, claim_expires)) in mutable.into_iter().enumerate() {
        let task = TaskId::new(harness.ids.as_ref());
        harness.seed_task(task).await;
        let source = harness.seed_run(task, RunState::Created).await;
        let target = harness.seed_run(task, state).await;
        if let Some(expires) = claim_expires {
            harness.seed_claim(target, expires).await;
        }
        harness
            .add(source, target, DependencyCondition::AnyTerminal, 0)
            .await
            .unwrap_or_else(|error| {
                panic!("case {index} ({state:?}, expired claim {claim_expires:?}): {error}")
            });
        assert_eq!(harness.dependencies(task).await.len(), 1, "case {index}");
    }

    let immutable = [
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
    for state in immutable {
        let task = TaskId::new(harness.ids.as_ref());
        harness.seed_task(task).await;
        let source = harness.seed_run(task, RunState::Created).await;
        let target = harness.seed_run(task, state).await;
        let error = match harness
            .add(source, target, DependencyCondition::AnyTerminal, 0)
            .await
        {
            Ok(()) => panic!("{state:?} must not be a mutable target"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::FailedPrecondition, "{state:?}");
        assert_eq!(error.retry_class(), RetryClass::Never, "{state:?}");
        assert!(harness.dependencies(task).await.is_empty(), "{state:?}");
        assert_eq!(
            harness
                .head(task)
                .await
                .expect("head present")
                .graph_revision,
            0,
            "{state:?}"
        );
    }

    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let claimed = harness.seed_run(task, RunState::Ready).await;
    harness.seed_claim(claimed, SEED_MS + 1_000).await;
    let events_before = harness.events().await.len();
    let error = harness
        .add(source, claimed, DependencyCondition::AnyTerminal, 0)
        .await
        .expect_err("a live claim freezes the target");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.dependencies(task).await.is_empty());
    assert_eq!(harness.events().await.len(), events_before);
}

#[tokio::test]
async fn cycle_forming_edges_are_rejected() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let first = harness.seed_run(task, RunState::Created).await;
    let second = harness.seed_run(task, RunState::Created).await;
    let third = harness.seed_run(task, RunState::Created).await;

    harness
        .add(first, second, DependencyCondition::CompletedSuccessfully, 0)
        .await
        .expect("first edge commits");
    harness
        .add(second, third, DependencyCondition::CompletedSuccessfully, 1)
        .await
        .expect("second edge commits");

    for (source, target) in [(third, first), (second, first)] {
        let error = harness
            .add(
                source,
                target,
                DependencyCondition::CompletedSuccessfully,
                2,
            )
            .await
            .expect_err("a cycle-forming edge is rejected");
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.retry_class(), RetryClass::Never);
    }

    assert_eq!(harness.dependencies(task).await.len(), 2);
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        2
    );
    let events = harness.events().await;
    assert_eq!(events.len(), 2, "rejections stage no events");
    harness.assert_acyclic(&[first, second, third]).await;
}

#[tokio::test]
async fn duplicate_edge_is_a_deterministic_no_op() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    harness
        .add(
            source,
            target,
            DependencyCondition::CompletedSuccessfully,
            0,
        )
        .await
        .expect("first edge commits");

    harness
        .add(
            source,
            target,
            DependencyCondition::CompletedSuccessfully,
            1,
        )
        .await
        .expect("identical duplicate is idempotent");
    assert_eq!(harness.dependencies(task).await.len(), 1);
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        1,
        "a duplicate does not advance the head"
    );
    assert_eq!(
        harness.events().await.len(),
        1,
        "a duplicate stages nothing"
    );

    let differing = harness
        .add(source, target, DependencyCondition::AnyTerminal, 1)
        .await
        .expect_err("a duplicate with another condition conflicts");
    assert_eq!(differing.code(), ErrorCode::Conflict);
    assert_eq!(differing.retry_class(), RetryClass::Never);

    let stale = harness
        .add(
            source,
            target,
            DependencyCondition::CompletedSuccessfully,
            0,
        )
        .await
        .expect_err("a stale expected revision conflicts");
    assert_eq!(stale.code(), ErrorCode::Conflict);
    assert_eq!(stale.retry_class(), RetryClass::Never);

    assert_eq!(harness.dependencies(task).await.len(), 1);
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        1
    );
    assert_eq!(harness.events().await.len(), 1);
}

#[tokio::test]
async fn stale_expected_revision_is_rejected_without_mutation() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    let error = harness
        .add(source, target, DependencyCondition::AnyTerminal, 7)
        .await
        .expect_err("a future revision is stale");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.dependencies(task).await.is_empty());
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        0
    );
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn missing_runs_are_not_found() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let present = harness.seed_run(task, RunState::Created).await;
    let absent = RunId::new(harness.ids.as_ref());

    let missing_target = harness
        .add(present, absent, DependencyCondition::AnyTerminal, 0)
        .await
        .expect_err("the target must exist");
    assert_eq!(missing_target.code(), ErrorCode::NotFound);
    assert_eq!(missing_target.retry_class(), RetryClass::Never);

    let missing_source = harness
        .add(absent, present, DependencyCondition::AnyTerminal, 0)
        .await
        .expect_err("the source must exist");
    assert_eq!(missing_source.code(), ErrorCode::NotFound);
    assert_eq!(missing_source.retry_class(), RetryClass::Never);

    assert!(harness.dependencies(task).await.is_empty());
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn unspecified_condition_is_rejected() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let source = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    let error = harness
        .add(source, target, DependencyCondition::Unspecified, 0)
        .await
        .expect_err("an unspecified condition is rejected");

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.dependencies(task).await.is_empty());
    assert!(harness.events().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opposite_edge_race_commits_at_most_one() {
    let harness = Arc::new(Harness::new().await);
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let left = harness.seed_run(task, RunState::Created).await;
    let right = harness.seed_run(task, RunState::Created).await;

    let barrier = Arc::new(Barrier::new(2));
    let mut writers = Vec::new();
    for (source, target) in [(left, right), (right, left)] {
        let harness = Arc::clone(&harness);
        let barrier = Arc::clone(&barrier);
        writers.push(tokio::spawn(async move {
            barrier.wait().await;
            harness
                .add(source, target, DependencyCondition::AnyTerminal, 0)
                .await
        }));
    }

    let mut committed = 0;
    let mut rejected = 0;
    for writer in writers {
        match writer.await.expect("writer joins") {
            Ok(()) => committed += 1,
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::Conflict);
                assert_eq!(error.retry_class(), RetryClass::Never);
                rejected += 1;
            }
        }
    }
    assert_eq!(committed, 1, "at most one of the opposite pair commits");
    assert_eq!(rejected, 1);

    let dependencies = harness.dependencies(task).await;
    assert_eq!(dependencies.len(), 1);
    assert!(
        (dependencies[0].source_run_id == left && dependencies[0].target_run_id == right)
            || (dependencies[0].source_run_id == right && dependencies[0].target_run_id == left),
        "the surviving edge is one of the racing pair"
    );
    assert_eq!(
        harness
            .head(task)
            .await
            .expect("head present")
            .graph_revision,
        1
    );
    assert_eq!(harness.events().await.len(), 1);
    harness.assert_acyclic(&[left, right]).await;
}

#[tokio::test]
async fn descendants_follow_parent_linkage_without_an_ancestry_table() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let root = harness.seed_run(task, RunState::Created).await;
    let child = harness.seed_child(task, root).await;
    let grandchild = harness.seed_child(task, child).await;
    let sibling = harness.seed_child(task, root).await;

    harness
        .add(child, sibling, DependencyCondition::AnyTerminal, 0)
        .await
        .expect("a dependency edge does not create ancestry");

    assert_eq!(
        harness.descendants(root).await.expect("descendants walk"),
        vec![child, sibling, grandchild],
        "descendants derive from runs.parent_run_id, dependency edges do not add children"
    );
    assert!(
        harness
            .descendants(grandchild)
            .await
            .expect("leaf walk")
            .is_empty()
    );

    let absent = RunId::new(harness.ids.as_ref());
    let error = harness
        .descendants(absent)
        .await
        .expect_err("unknown roots are not found");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn second_task_descendants_do_not_leak_across_task_boundaries() {
    let harness = Harness::new().await;
    let left_task = TaskId::new(harness.ids.as_ref());
    let right_task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(left_task).await;
    harness.seed_task(right_task).await;
    let root = harness.seed_run(left_task, RunState::Created).await;
    let child = harness.seed_child(left_task, root).await;
    let _elsewhere = harness.seed_run(right_task, RunState::Created).await;

    assert_eq!(
        harness.descendants(root).await.expect("descendants walk"),
        vec![child]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn sequential_inserts_keep_the_graph_acyclic(
        pairs in prop::collection::vec((0u8..5u8, 0u8..5u8), 0..24),
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .expect("test runtime builds");
        runtime.block_on(async move {
            let harness = Harness::new().await;
            let task = TaskId::new(harness.ids.as_ref());
            harness.seed_task(task).await;
            let mut runs = Vec::new();
            for _ in 0..5 {
                runs.push(harness.seed_run(task, RunState::Created).await);
            }

            for (source_index, target_index) in pairs {
                let source = runs[source_index as usize];
                let target = runs[target_index as usize];
                let mut txn = harness.write().await;
                let expected = txn
                    .graph()
                    .get_head(task)
                    .await
                    .expect("head read")
                    .expect("head present")
                    .graph_revision;
                let result = add_dependency(
                    txn.as_mut(),
                    source,
                    target,
                    DependencyCondition::CompletedSuccessfully,
                    expected,
                    SEED_MS,
                )
                .await;
                match result {
                    Ok(()) => txn.commit().await.expect("edge commits"),
                    Err(error) => {
                        assert_eq!(error.retry_class(), RetryClass::Never);
                        txn.rollback().await.expect("edge rolls back");
                    }
                }
            }

            let dependencies = harness.dependencies(task).await;
            let head = harness
                .head(task)
                .await
                .expect("head present")
                .graph_revision;
            assert_eq!(
                dependencies.len() as u64,
                head,
                "each committed edge advances the head exactly once"
            );
            harness.assert_acyclic(&runs).await;
        });
    }
}
