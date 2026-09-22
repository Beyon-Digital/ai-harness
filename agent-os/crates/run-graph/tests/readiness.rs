//! Readiness acceptance tests (R4.1-R4.3): the three dependency conditions
//! are exact over every run state, and every dependency edge into a target
//! must be satisfied by its source run's persisted state.

use std::sync::Arc;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
};
use domain::resource::DependencyCondition;
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewTask, RunCas, RunPatch};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use run_graph::graph::add_dependency;
use run_graph::readiness::{condition_met, dependencies_satisfied};
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";

const ALL_STATES: [RunState; 12] = [
    RunState::Unspecified,
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
                task_kind: "readiness".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.graph().ensure_head(task_id).await.expect("seed head");
        txn.commit().await.expect("seed task commits");
    }

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
        txn.commit().await.expect("edge transaction commits");
    }

    async fn satisfied(&self, target: RunId) -> errors::Result<bool> {
        let mut txn = self.write().await;
        let result = dependencies_satisfied(txn.as_mut(), target).await;
        txn.rollback().await.expect("readiness rollback");
        result
    }
}

#[test]
fn condition_met_matrix_is_exact_for_every_state() {
    for state in ALL_STATES {
        assert_eq!(
            condition_met(DependencyCondition::CompletedSuccessfully, state),
            state == RunState::Completed,
            "completed_successfully for {state:?}"
        );
        assert_eq!(
            condition_met(DependencyCondition::AnyTerminal, state),
            matches!(
                state,
                RunState::Completed | RunState::Failed | RunState::Cancelled
            ),
            "any_terminal for {state:?}"
        );
        assert_eq!(
            condition_met(DependencyCondition::CompletedOrCancelled, state),
            matches!(state, RunState::Completed | RunState::Cancelled),
            "completed_or_cancelled for {state:?}"
        );
        assert!(
            !condition_met(DependencyCondition::Unspecified, state),
            "unspecified never satisfies ({state:?})"
        );
    }
}

#[tokio::test]
async fn runs_without_dependencies_are_satisfied() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let target = harness.seed_run(task, RunState::Created).await;
    assert!(
        harness.satisfied(target).await.expect("readiness read"),
        "a run without dependency edges is ready"
    );
}

#[tokio::test]
async fn dependency_conditions_are_honored_over_persisted_states() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;

    let cases = [
        (
            DependencyCondition::CompletedSuccessfully,
            RunState::Completed,
            true,
        ),
        (
            DependencyCondition::CompletedSuccessfully,
            RunState::Failed,
            false,
        ),
        (
            DependencyCondition::CompletedSuccessfully,
            RunState::Cancelled,
            false,
        ),
        (
            DependencyCondition::CompletedSuccessfully,
            RunState::Running,
            false,
        ),
        (DependencyCondition::AnyTerminal, RunState::Completed, true),
        (DependencyCondition::AnyTerminal, RunState::Failed, true),
        (DependencyCondition::AnyTerminal, RunState::Cancelled, true),
        (DependencyCondition::AnyTerminal, RunState::Running, false),
        (
            DependencyCondition::AnyTerminal,
            RunState::WaitingTool,
            false,
        ),
        (
            DependencyCondition::CompletedOrCancelled,
            RunState::Completed,
            true,
        ),
        (
            DependencyCondition::CompletedOrCancelled,
            RunState::Cancelled,
            true,
        ),
        (
            DependencyCondition::CompletedOrCancelled,
            RunState::Failed,
            false,
        ),
        (
            DependencyCondition::CompletedOrCancelled,
            RunState::Ready,
            false,
        ),
    ];

    for (index, (condition, source_state, expected)) in cases.into_iter().enumerate() {
        let source = harness.seed_run(task, source_state).await;
        let target = harness.seed_run(task, RunState::Created).await;
        harness.add(source, target, condition, index as u64).await;
        assert_eq!(
            harness.satisfied(target).await.expect("readiness read"),
            expected,
            "case {index}: {condition:?} over {source_state:?}"
        );
    }
}

#[tokio::test]
async fn one_unmet_dependency_blocks_satisfaction_until_it_terminalizes() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let exhausted = harness.seed_run(task, RunState::Running).await;
    let completed = harness.seed_run(task, RunState::Completed).await;
    let target = harness.seed_run(task, RunState::Ready).await;

    harness
        .add(exhausted, target, DependencyCondition::AnyTerminal, 0)
        .await;
    harness
        .add(
            completed,
            target,
            DependencyCondition::CompletedSuccessfully,
            1,
        )
        .await;
    assert!(
        !harness.satisfied(target).await.expect("readiness read"),
        "a running any_terminal source blocks the target"
    );

    harness.set_state(exhausted, RunState::Failed).await;
    assert!(
        harness.satisfied(target).await.expect("readiness read"),
        "the target is satisfied once every source is terminal"
    );
}

#[tokio::test]
async fn dependencies_targeting_other_runs_do_not_affect_the_target() {
    let harness = Harness::new().await;
    let task = TaskId::new(harness.ids.as_ref());
    harness.seed_task(task).await;
    let blocked_source = harness.seed_run(task, RunState::Running).await;
    let other_target = harness.seed_run(task, RunState::Created).await;
    let target = harness.seed_run(task, RunState::Created).await;

    harness
        .add(
            blocked_source,
            other_target,
            DependencyCondition::AnyTerminal,
            0,
        )
        .await;
    assert!(
        harness.satisfied(target).await.expect("readiness read"),
        "edges into another target do not gate this one"
    );
    assert!(
        !harness
            .satisfied(other_target)
            .await
            .expect("readiness read")
    );
}

#[tokio::test]
async fn missing_targets_are_not_found() {
    let harness = Harness::new().await;
    let absent = RunId::new(harness.ids.as_ref());
    let error = harness
        .satisfied(absent)
        .await
        .expect_err("unknown targets are not found");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);
}
