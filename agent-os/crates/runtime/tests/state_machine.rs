//! Run state machine acceptance tests (R2): the normative transition table for
//! every state pair, revisioned commits against a real SQLite store, terminal
//! reasons, and recovery-disposition independence.

use std::sync::Arc;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewTask, RunCas, RunPatch, RunRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use proptest::prelude::*;
use runtime::{run, state};
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

/// Independent encoding of `specs/runtime-manager.md`; the implementation table
/// must agree with this for every pair.
fn normative_allows(from: RunState, to: RunState) -> bool {
    use RunState::*;
    matches!(
        (from, to),
        (Created, Ready)
            | (Created, Cancelled) // cancellation of a never-started run
            | (Ready, Running)
            | (Ready, Cancelled)
            | (Running, WaitingTool)
            | (Running, WaitingChild)
            | (Running, WaitingHuman)
            | (Running, Suspended)
            | (Running, Cancelling)
            | (Running, Completed)
            | (Running, Failed)
            | (WaitingTool, Running)
            | (WaitingTool, Cancelling)
            | (WaitingTool, Failed)
            | (WaitingChild, Running)
            | (WaitingChild, Cancelling)
            | (WaitingChild, Failed)
            | (WaitingHuman, Running)
            | (WaitingHuman, Cancelling)
            | (WaitingHuman, Failed)
            | (Suspended, Running)
            | (Suspended, Cancelling)
            | (Suspended, Failed)
            | (Cancelling, Cancelled)
            | (Cancelling, Failed)
    )
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: DeterministicIds,
    fence_epoch: u64,
    task_id: TaskId,
    principal: PrincipalId,
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
        let ids = DeterministicIds::new(SEED_MS);
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(&ids))
            .await
            .expect("daemon fence acquired");
        let principal = PrincipalId::new(&ids);
        let task_id = TaskId::new(&ids);
        let mut txn = store
            .begin_write(TxContext {
                daemon_epoch: fence.epoch.0,
                principal_id: principal,
                command_id: CommandId::new(&ids),
                correlation_id: None,
            })
            .await
            .expect("write transaction opens");
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: ActorId::new(&ids),
                task_kind: "state-machine".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("task seeds");
        txn.commit().await.expect("task seed commits");
        Self {
            _dir: dir,
            store,
            ids,
            fence_epoch: fence.epoch.0,
            task_id,
            principal,
        }
    }

    async fn write_txn(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(TxContext {
                daemon_epoch: self.fence_epoch,
                principal_id: self.principal,
                command_id: CommandId::new(&self.ids),
                correlation_id: None,
            })
            .await
            .expect("write transaction opens")
    }

    async fn seed_run(&self, run_id: RunId, state: RunState) -> RunRow {
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("run stream key"),
            0,
        );
        let mut txn = self.write_txn().await;
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id: self.task_id,
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
            .expect("run seeds");
        txn.commit().await.expect("run seed commits");
        self.run(run_id).await
    }

    async fn run(&self, run_id: RunId) -> RunRow {
        let mut txn = self.write_txn().await;
        let row = txn
            .runs()
            .get(run_id)
            .await
            .expect("run read")
            .expect("seeded run exists");
        txn.rollback().await.expect("read rollback");
        row
    }

    /// Commits even when the transition is rejected, so a mutation smuggled
    /// alongside an error would be observed by the next read.
    async fn attempt(
        &self,
        run_id: RunId,
        expect_revision: u64,
        to: RunState,
        reason: Option<String>,
    ) -> errors::Result<RunRow> {
        let mut txn = self.write_txn().await;
        let result =
            run::transition(txn.as_mut(), run_id, expect_revision, to, reason, SEED_MS).await;
        txn.commit()
            .await
            .expect("attempt transaction commits regardless of outcome");
        result
    }

    async fn set_recovery(
        &self,
        run_id: RunId,
        expect_revision: u64,
        recovery: RecoveryDisposition,
    ) -> bool {
        let mut txn = self.write_txn().await;
        let updated = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: expect_revision,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    recovery: Some(recovery),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await
            .expect("recovery patch applies");
        txn.commit().await.expect("recovery patch commits");
        updated
    }
}

#[test]
fn normative_table_is_matched_for_every_state_pair() {
    for from in ALL_STATES {
        for to in ALL_STATES {
            assert_eq!(
                state::allows(from, to),
                normative_allows(from, to),
                "table disagrees for {from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn terminal_states_are_exactly_completed_failed_and_cancelled() {
    for candidate in ALL_STATES {
        let expected = matches!(
            candidate,
            RunState::Completed | RunState::Failed | RunState::Cancelled
        );
        assert_eq!(state::is_terminal(candidate), expected, "{candidate:?}");
    }
}

#[tokio::test]
async fn every_allowed_pair_commits_once_and_persists_the_target() {
    let harness = Harness::new().await;
    let mut checked = 0_u32;
    for from in PERSISTED_STATES {
        for to in ALL_STATES {
            if !normative_allows(from, to) {
                continue;
            }
            checked += 1;
            let run_id = RunId::new(&harness.ids);
            let seeded = harness.seed_run(run_id, from).await;
            assert_eq!(seeded.run_revision, 0);

            let updated = harness
                .attempt(run_id, 0, to, None)
                .await
                .unwrap_or_else(|error| panic!("{from:?} -> {to:?} rejected: {error}"));

            assert_eq!(updated.state, to, "{from:?} -> {to:?} target");
            assert_eq!(updated.run_revision, 1, "{from:?} -> {to:?} revision");

            let persisted = harness.run(run_id).await;
            assert_eq!(persisted.state, to);
            assert_eq!(persisted.run_revision, 1);
            assert_eq!(persisted.recovery, RecoveryDisposition::Normal);
            assert_eq!(persisted.claim_owner, None);
            assert_eq!(persisted.cancellation_epoch, 0);
            let expected_reason = match to {
                RunState::Completed => Some(state::REASON_COMPLETED),
                RunState::Failed => Some(state::REASON_FAILED),
                RunState::Cancelled => Some(state::REASON_CANCELLED),
                _ => None,
            };
            assert_eq!(persisted.terminal_reason.as_deref(), expected_reason);
        }
    }
    assert_eq!(checked, 25, "the normative table has 25 allowed pairs");
}

#[tokio::test]
async fn every_forbidden_pair_conflicts_without_mutation() {
    let harness = Harness::new().await;
    for from in PERSISTED_STATES {
        let run_id = RunId::new(&harness.ids);
        harness.seed_run(run_id, from).await;
        for to in ALL_STATES {
            if normative_allows(from, to) {
                continue;
            }
            let error = harness
                .attempt(run_id, 0, to, None)
                .await
                .err()
                .unwrap_or_else(|| panic!("{from:?} -> {to:?} was accepted"));
            assert_eq!(error.code(), ErrorCode::Conflict, "{from:?} -> {to:?}");
            assert_eq!(error.retry_class(), RetryClass::Never, "{from:?} -> {to:?}");

            let persisted = harness.run(run_id).await;
            assert_eq!(persisted.state, from, "{from:?} -> {to:?} state");
            assert_eq!(persisted.run_revision, 0, "{from:?} -> {to:?} revision");
            assert_eq!(persisted.terminal_reason, None);
        }
    }
}

#[tokio::test]
async fn cancellation_moves_a_never_started_run_directly_to_cancelled() {
    let harness = Harness::new().await;
    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Created).await;

    let updated = harness
        .attempt(run_id, 0, RunState::Cancelled, None)
        .await
        .expect("Created -> Cancelled commits for cancellation");

    assert_eq!(updated.state, RunState::Cancelled);
    assert_eq!(
        updated.run_revision, 1,
        "the cancellation-only edge bumps the revision once"
    );
    let persisted = harness.run(run_id).await;
    assert_eq!(persisted.state, RunState::Cancelled);
    assert_eq!(persisted.run_revision, 1);
    assert_eq!(
        persisted.terminal_reason.as_deref(),
        Some(state::REASON_CANCELLED)
    );
}

#[tokio::test]
async fn terminal_states_reject_further_transitions_and_keep_the_reason() {
    let harness = Harness::new().await;
    for (steps, terminal, expected_reason) in [
        (
            vec![RunState::Completed],
            RunState::Completed,
            state::REASON_COMPLETED,
        ),
        (
            vec![RunState::Failed],
            RunState::Failed,
            state::REASON_FAILED,
        ),
        (
            vec![RunState::Cancelling, RunState::Cancelled],
            RunState::Cancelled,
            state::REASON_CANCELLED,
        ),
    ] {
        let run_id = RunId::new(&harness.ids);
        harness.seed_run(run_id, RunState::Running).await;
        let mut revision = 0_u64;
        for step in &steps {
            let reached = harness
                .attempt(run_id, revision, *step, None)
                .await
                .expect("terminal path commits");
            revision += 1;
            assert_eq!(reached.run_revision, revision);
        }
        let reached = harness.run(run_id).await;
        assert_eq!(reached.terminal_reason.as_deref(), Some(expected_reason));

        for target in ALL_STATES {
            let error = harness
                .attempt(run_id, revision, target, Some("second attempt".to_owned()))
                .await
                .err()
                .unwrap_or_else(|| panic!("{terminal:?} -> {target:?} was accepted"));
            assert_eq!(error.code(), ErrorCode::Conflict);
            assert_eq!(error.retry_class(), RetryClass::Never);
        }

        let persisted = harness.run(run_id).await;
        assert_eq!(persisted.state, terminal);
        assert_eq!(persisted.run_revision, revision);
        assert_eq!(persisted.terminal_reason.as_deref(), Some(expected_reason));
    }
}

#[tokio::test]
async fn stale_and_future_revisions_conflict_without_mutation() {
    let harness = Harness::new().await;
    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Created).await;
    let advanced = harness
        .attempt(run_id, 0, RunState::Ready, None)
        .await
        .expect("first transition commits");
    assert_eq!(advanced.run_revision, 1);

    for expect_revision in [0_u64, 7] {
        let error = harness
            .attempt(run_id, expect_revision, RunState::Running, None)
            .await
            .err()
            .unwrap_or_else(|| panic!("revision {expect_revision} was accepted"));
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.retry_class(), RetryClass::Never);
    }

    let persisted = harness.run(run_id).await;
    assert_eq!(persisted.state, RunState::Ready);
    assert_eq!(persisted.run_revision, 1);
}

#[tokio::test]
async fn missing_runs_are_not_found() {
    let harness = Harness::new().await;
    let missing = RunId::new(&harness.ids);
    let error = harness
        .attempt(missing, 0, RunState::Ready, None)
        .await
        .expect_err("missing run rejects");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn sequential_transitions_increment_revision_once_per_commit() {
    let harness = Harness::new().await;
    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Created).await;
    let path = [
        RunState::Ready,
        RunState::Running,
        RunState::WaitingChild,
        RunState::Running,
        RunState::Cancelling,
        RunState::Cancelled,
    ];
    let mut revision = 0_u64;
    for to in path {
        revision += 1;
        let updated = harness
            .attempt(run_id, revision - 1, to, None)
            .await
            .unwrap_or_else(|error| panic!("{to:?} rejected: {error}"));
        assert_eq!(updated.run_revision, revision);
        assert_eq!(updated.state, to);
    }
    let persisted = harness.run(run_id).await;
    assert_eq!(persisted.run_revision, 6);
    assert_eq!(persisted.state, RunState::Cancelled);
    assert_eq!(
        persisted.terminal_reason.as_deref(),
        Some(state::REASON_CANCELLED)
    );
}

#[tokio::test]
async fn terminal_reason_defaults_are_stable_and_caller_reasons_win() {
    let harness = Harness::new().await;
    for (from, to, expected) in [
        (
            RunState::Running,
            RunState::Completed,
            state::REASON_COMPLETED,
        ),
        (RunState::Running, RunState::Failed, state::REASON_FAILED),
        (
            RunState::Ready,
            RunState::Cancelled,
            state::REASON_CANCELLED,
        ),
    ] {
        let run_id = RunId::new(&harness.ids);
        harness.seed_run(run_id, from).await;
        let updated = harness
            .attempt(run_id, 0, to, None)
            .await
            .expect("terminal transition commits");
        assert_eq!(updated.terminal_reason.as_deref(), Some(expected));
    }

    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Running).await;
    let updated = harness
        .attempt(
            run_id,
            0,
            RunState::Failed,
            Some("provider exploded".to_owned()),
        )
        .await
        .expect("failure transition commits");
    assert_eq!(
        updated.terminal_reason.as_deref(),
        Some("provider exploded")
    );

    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Running).await;
    let updated = harness
        .attempt(
            run_id,
            0,
            RunState::Cancelling,
            Some(state::REASON_CANCELLATION_REQUESTED.to_owned()),
        )
        .await
        .expect("cancellation request commits");
    assert_eq!(updated.terminal_reason, None);

    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Running).await;
    let updated = harness
        .attempt(
            run_id,
            0,
            RunState::WaitingTool,
            Some("not terminal".to_owned()),
        )
        .await
        .expect("waiting transition commits");
    assert_eq!(updated.terminal_reason, None);
}

#[tokio::test]
async fn recovery_disposition_changes_leave_state_and_run_state_untouched() {
    let harness = Harness::new().await;
    let run_id = RunId::new(&harness.ids);
    harness.seed_run(run_id, RunState::Created).await;

    assert!(
        harness
            .set_recovery(run_id, 0, RecoveryDisposition::NeedsReconciliation)
            .await
    );
    let patched = harness.run(run_id).await;
    assert_eq!(patched.state, RunState::Created);
    assert_eq!(patched.recovery, RecoveryDisposition::NeedsReconciliation);
    assert_eq!(patched.run_revision, 1);
    assert_eq!(patched.terminal_reason, None);

    assert!(
        !harness
            .set_recovery(run_id, 0, RecoveryDisposition::BlockedUnknownEffect)
            .await,
        "a stale recovery patch must not commit"
    );
    let unchanged = harness.run(run_id).await;
    assert_eq!(unchanged.recovery, RecoveryDisposition::NeedsReconciliation);
    assert_eq!(unchanged.run_revision, 1);

    let updated = harness
        .attempt(run_id, 1, RunState::Ready, None)
        .await
        .expect("transition after recovery patch commits");
    assert_eq!(updated.state, RunState::Ready);
    assert_eq!(updated.recovery, RecoveryDisposition::NeedsReconciliation);
    assert_eq!(updated.run_revision, 2);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn arbitrary_attempt_sequences_keep_revisions_monotonic_and_states_legal(
        initial in proptest::sample::select(PERSISTED_STATES.to_vec()),
        attempts in proptest::collection::vec(
            (
                proptest::sample::select(ALL_STATES.to_vec()),
                proptest::arbitrary::any::<bool>(),
                proptest::option::of(proptest::sample::select(vec![
                    String::new(),
                    "operator cancelled".to_owned(),
                    "provider timeout".to_owned(),
                ])),
            ),
            0..16,
        ),
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime builds");
        let violations = runtime.block_on(async move {
            let harness = Harness::new().await;
            let run_id = RunId::new(&harness.ids);
            let mut row = harness.seed_run(run_id, initial).await;
            let mut violations: Vec<String> = Vec::new();

            for (target, stale, reason) in attempts {
                let before = row;
                let expect_revision = if stale {
                    before.run_revision.wrapping_sub(1)
                } else {
                    before.run_revision
                };
                let result = harness
                    .attempt(run_id, expect_revision, target, reason)
                    .await;
                row = harness.run(run_id).await;

                if row.run_revision < before.run_revision {
                    violations.push(format!(
                        "revision regressed from {} to {}",
                        before.run_revision, row.run_revision
                    ));
                }
                match result {
                    Ok(updated) => {
                        if updated.state != target || row.state != target {
                            violations.push(format!(
                                "accepted transition did not persist {target:?}"
                            ));
                        }
                        if updated.run_revision != before.run_revision + 1
                            || row.run_revision != before.run_revision + 1
                        {
                            violations.push(format!(
                                "accepted transition from revision {} did not increment once (persisted {})",
                                before.run_revision, row.run_revision
                            ));
                        }
                        if !state::allows(before.state, target) {
                            violations.push(format!(
                                "illegal transition {:?} -> {target:?} was accepted",
                                before.state
                            ));
                        }
                        if expect_revision != before.run_revision {
                            violations.push(format!(
                                "transition was accepted with stale revision {expect_revision} at {}",
                                before.run_revision
                            ));
                        }
                        let terminal = state::is_terminal(target);
                        if terminal != row.terminal_reason.is_some() {
                            violations.push(format!(
                                "terminal reason mismatch for {target:?}: {:?}",
                                row.terminal_reason
                            ));
                        }
                    }
                    Err(error) => {
                        if !matches!(error.code(), ErrorCode::Conflict | ErrorCode::NotFound) {
                            violations
                                .push(format!("unexpected error code {:?}", error.code()));
                        }
                        if error.retry_class() != RetryClass::Never {
                            violations.push("rejected transition was retryable".to_owned());
                        }
                        if row.state != before.state
                            || row.run_revision != before.run_revision
                            || row.terminal_reason != before.terminal_reason
                        {
                            violations.push(format!(
                                "rejected transition mutated the run: {:?} rev {} reason {:?} -> {:?} rev {} reason {:?}",
                                before.state,
                                before.run_revision,
                                before.terminal_reason,
                                row.state,
                                row.run_revision,
                                row.terminal_reason
                            ));
                        }
                        if state::allows(before.state, target)
                            && expect_revision == before.run_revision
                        {
                            violations.push(format!(
                                "legal transition {:?} -> {target:?} was rejected",
                                before.state
                            ));
                        }
                    }
                }
            }
            violations
        });
        prop_assert!(
            violations.is_empty(),
            "property violations: {}",
            violations.join("; ")
        );
    }
}
