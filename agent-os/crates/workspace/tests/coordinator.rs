//! WRK-003: lease acquisition, exclusive transfer with epoch fencing,
//! parallel forks, explicit merge, and shared-mode fail-closed.

use std::path::Path;
use std::process::Command;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
    WorkspaceId,
};
use domain::resource::{LeaseEnforcementState, WorkspaceAccessMode};
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::ErrorCode;
use kernel_store::models::{NewRun, NewTask};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;
use workspace::{coordinator, local};

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git")
        .status
        .success()
}

fn must_git(dir: &Path, args: &[&str]) {
    assert!(git(dir, args), "git {args:?} failed");
}

fn init_repo(dir: &Path) {
    must_git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("shared.txt"), "base\n").expect("write");
    must_git(dir, &["add", "."]);
    must_git(dir, &["commit", "-q", "-m", "c1"]);
}

async fn open() -> (SqliteKernelStore, u64, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("kernel.db"),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store");
    let ids = DeterministicIds::new(SEED);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence");
    (store, fence.epoch.0, dir)
}

async fn txn<'s>(store: &'s SqliteKernelStore, epoch: u64) -> Box<dyn KernelTxn + 's> {
    let ids = DeterministicIds::new(SEED + 1);
    store
        .begin_write(TxContext {
            daemon_epoch: epoch,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn")
}

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0)
}

/// Inserts task + run rows so lease owner FKs resolve.
async fn run_row(txn: &mut dyn KernelTxn, run_id: RunId, ids: &testkit::ids::DeterministicIds) {
    let task_id = TaskId::new(ids);
    txn.tasks()
        .insert(NewTask {
            task_id,
            session_id: None,
            created_by_actor_id: ActorId::new(ids),
            task_kind: "test".into(),
            payload: Vec::new(),
            created_at_ms: NOW,
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
            loop_epoch: 1,
            step_sequence: 0,
            input_event_cursor: cursor(run_id),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            created_at_ms: NOW,
        })
        .await
        .expect("run");
}

#[tokio::test]
async fn exclusive_transfer_stale_token_rejected() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 2);
    let (ws, parent, child) = (WorkspaceId::new(&ids), RunId::new(&ids), RunId::new(&ids));
    let root = tempfile::tempdir().expect("root");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, ws, root.path(), NOW)
        .await
        .expect("ws");
    run_row(&mut *tx, parent, &ids).await;
    run_row(&mut *tx, child, &ids).await;
    let lease = coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        parent,
        WorkspaceAccessMode::ExclusiveWrite,
        &[],
        Vec::new(),
        NOW,
    )
    .await
    .expect("acquire");
    assert_eq!(lease.lease_epoch, 1);

    let moved = coordinator::transfer_exclusive(
        &mut *tx,
        lease.lease_id,
        parent,
        child,
        lease.lease_epoch,
        b"delegation-proof".to_vec(),
    )
    .await
    .expect("transfer");
    assert_eq!(moved.owner_run_id, child);
    assert_eq!(moved.lease_epoch, 2);
    tx.commit().await.expect("commit");

    // The parent's old epoch token must fail immediately post-transfer.
    let mut tx = txn(&store, epoch).await;
    let err = coordinator::verify_write_authority(&mut *tx, lease.lease_id, ws, parent, 1)
        .await
        .expect_err("old owner + old epoch is dead");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    // A second transfer at the stale epoch also fails.
    let err = coordinator::transfer_exclusive(
        &mut *tx,
        lease.lease_id,
        parent,
        RunId::new(&ids),
        1,
        Vec::new(),
    )
    .await
    .expect_err("stale epoch transfer");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    // New owner at the new epoch verifies.
    coordinator::verify_write_authority(&mut *tx, lease.lease_id, ws, child, 2)
        .await
        .expect("child holds authority");
}

#[tokio::test]
async fn two_active_exclusive_leases_impossible() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 3);
    let ws = WorkspaceId::new(&ids);
    let (r1, r2) = (RunId::new(&ids), RunId::new(&ids));
    let root = tempfile::tempdir().expect("root");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, ws, root.path(), NOW)
        .await
        .expect("ws");
    run_row(&mut *tx, r1, &ids).await;
    run_row(&mut *tx, r2, &ids).await;
    coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        r1,
        WorkspaceAccessMode::ExclusiveWrite,
        &[],
        Vec::new(),
        NOW,
    )
    .await
    .expect("first exclusive");
    // Same owner may hold many read leases, but a second ACTIVE exclusive
    // lease hits the partial unique index.
    coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        r2,
        WorkspaceAccessMode::ReadOnly,
        &[],
        Vec::new(),
        NOW,
    )
    .await
    .expect("read lease coexists");
    let err = coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        r2,
        WorkspaceAccessMode::ExclusiveWrite,
        &[],
        Vec::new(),
        NOW,
    )
    .await
    .expect_err("second exclusive rejected");
    assert_eq!(err.code(), ErrorCode::Conflict);
}

#[tokio::test]
async fn parallel_forks_are_independent() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 4);
    let parent_ws = WorkspaceId::new(&ids);
    let (child_a_ws, child_b_ws) = (WorkspaceId::new(&ids), WorkspaceId::new(&ids));
    let (run_a, run_b) = (RunId::new(&ids), RunId::new(&ids));

    let repo = tempfile::tempdir().expect("repo");
    init_repo(repo.path());

    let fork_a = tempfile::tempdir().expect("a").path().join("wa");
    let fork_b = tempfile::tempdir().expect("b").path().join("wb");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, parent_ws, repo.path(), NOW)
        .await
        .expect("parent ws");
    run_row(&mut *tx, run_a, &ids).await;
    run_row(&mut *tx, run_b, &ids).await;
    let lease_a = coordinator::fork_for_child(
        &mut *tx,
        &ids,
        repo.path(),
        parent_ws,
        child_a_ws,
        &fork_a,
        run_a,
        NOW,
    )
    .await
    .expect("fork a");
    let lease_b = coordinator::fork_for_child(
        &mut *tx,
        &ids,
        repo.path(),
        parent_ws,
        child_b_ws,
        &fork_b,
        run_b,
        NOW,
    )
    .await
    .expect("fork b");
    tx.commit().await.expect("commit");

    // Each child has its own exclusive lease on its own fork.
    assert_eq!(lease_a.workspace_id, child_a_ws);
    assert_eq!(lease_b.workspace_id, child_b_ws);

    // Writes in A never appear in B or the parent.
    local::write(&fork_a, "a-only.txt", b"from a").expect("write a");
    local::write(&fork_b, "shared.txt", b"from b").expect("write b");
    assert!(!fork_a.join("shared.txt").to_string_lossy().is_empty());
    assert_eq!(
        std::fs::read_to_string(fork_b.join("shared.txt")).expect("read"),
        "from b"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("shared.txt")).expect("parent"),
        "base\n"
    );
    assert!(!fork_b.join("a-only.txt").exists());
}

#[tokio::test]
async fn merge_applies_clean_and_surfaces_conflicts() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 5);
    let parent_ws = WorkspaceId::new(&ids);
    let child_ws = WorkspaceId::new(&ids);
    let run = RunId::new(&ids);

    let repo = tempfile::tempdir().expect("repo");
    init_repo(repo.path());
    let child_root = tempfile::tempdir().expect("c").path().join("wc");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, parent_ws, repo.path(), NOW)
        .await
        .expect("ws");
    run_row(&mut *tx, run, &ids).await;
    coordinator::fork_for_child(
        &mut *tx,
        &ids,
        repo.path(),
        parent_ws,
        child_ws,
        &child_root,
        run,
        NOW,
    )
    .await
    .expect("fork");
    tx.commit().await.expect("commit");

    // Clean merge: child adds a file the parent never touched.
    local::write(&child_root, "child.txt", b"new work").expect("write");
    must_git(&child_root, &["add", "."]);
    must_git(&child_root, &["commit", "-q", "-m", "child work"]);
    let mut tx = txn(&store, epoch).await;
    coordinator::merge(&mut *tx, repo.path(), &child_root)
        .await
        .expect("clean merge");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("child.txt")).expect("merged"),
        "new work"
    );
    // The merge is staged, not committed — caller inspects then commits.
    must_git(repo.path(), &["commit", "-q", "-m", "merge child"]);
    tx.commit().await.expect("commit");

    // Conflict: second child edits the same line as the parent.
    let child2_ws = WorkspaceId::new(&ids);
    let child2_root = tempfile::tempdir().expect("c2").path().join("wc2");
    let mut tx = txn(&store, epoch).await;
    coordinator::fork_for_child(
        &mut *tx,
        &ids,
        repo.path(),
        parent_ws,
        child2_ws,
        &child2_root,
        run,
        NOW,
    )
    .await
    .expect("fork2");
    tx.commit().await.expect("commit");
    std::fs::write(child2_root.join("shared.txt"), "child edit\n").expect("w");
    must_git(&child2_root, &["add", "."]);
    must_git(&child2_root, &["commit", "-q", "-m", "child edit"]);
    std::fs::write(repo.path().join("shared.txt"), "parent edit\n").expect("w");
    must_git(repo.path(), &["add", "."]);
    must_git(repo.path(), &["commit", "-q", "-m", "parent edit"]);

    let mut tx = txn(&store, epoch).await;
    let err = coordinator::merge(&mut *tx, repo.path(), &child2_root)
        .await
        .expect_err("conflict surfaces");
    assert_eq!(err.code(), ErrorCode::Conflict);
    assert!(
        err.message().contains("shared.txt"),
        "conflict names the file: {}",
        err.message()
    );
    // Merge aborted: target is clean, no half-merge state.
    assert!(
        std::process::Command::new("git")
            .args(["-C"])
            .arg(repo.path())
            .args(["status", "--porcelain"])
            .output()
            .expect("status")
            .stdout
            .is_empty()
    );
}

#[tokio::test]
async fn shared_mode_rejected_by_local_adapter() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 6);
    let ws = WorkspaceId::new(&ids);
    let run = RunId::new(&ids);
    let root = tempfile::tempdir().expect("root");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, ws, root.path(), NOW)
        .await
        .expect("ws");
    run_row(&mut *tx, run, &ids).await;
    // The local adapter advertises no locking/version/conflict caps.
    let caps = local::capabilities(root.path());
    assert!(!caps.is_security_boundary);
    let advertised: Vec<String> = vec![];
    let err = coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        run,
        WorkspaceAccessMode::SharedCoordinatedWrite,
        &advertised,
        Vec::new(),
        NOW,
    )
    .await
    .expect_err("shared write needs locking caps");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);

    // With all three caps advertised it proceeds (foreign adapter case).
    let caps3 = vec![
        coordinator::shared_write_caps::LOCKING.to_owned(),
        coordinator::shared_write_caps::VERSIONED_WRITE.to_owned(),
        coordinator::shared_write_caps::CONFLICT_REPORT.to_owned(),
    ];
    let lease = coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        run,
        WorkspaceAccessMode::SharedCoordinatedWrite,
        &caps3,
        Vec::new(),
        NOW,
    )
    .await
    .expect("capable adapter may share");
    assert_eq!(lease.mode, WorkspaceAccessMode::SharedCoordinatedWrite);

    // Revoke path: stale epoch fails, current epoch succeeds.
    let err = coordinator::revoke_lease(&mut *tx, lease.lease_id, 99)
        .await
        .expect_err("stale epoch revoke");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    coordinator::revoke_lease(&mut *tx, lease.lease_id, lease.lease_epoch)
        .await
        .expect("revoke");
    let stored = tx
        .workspaces()
        .get_lease(lease.lease_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(stored.enforcement_state, LeaseEnforcementState::Revoked);
}
