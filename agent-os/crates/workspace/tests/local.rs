//! WRK-002: local workspace adapter — CRUD, Git worktree fork at exact
//! base revision, copy-fork capability honesty, path traversal denial.

use std::process::Command;

use domain::ids::{CommandId, DaemonInstanceId, PrincipalId, WorkspaceId};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;
use workspace::local;

const SEED: i64 = 1_700_000_000_000;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
}

fn init_repo(dir: &TempDir) -> String {
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "rev1").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "c1"]);
    local::base_revision(dir.path())
        .expect("rev")
        .expect("git rev")
}

/// Advances the repo HEAD one commit past `init_repo`'s base.
fn advance_head(dir: &TempDir) {
    std::fs::write(dir.path().join("a.txt"), "rev2-changed").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "c2"]);
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

async fn txn<'s>(
    store: &'s SqliteKernelStore,
    epoch: u64,
) -> Box<dyn kernel_store::KernelTxn + 's> {
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

#[test]
fn create_read_write_list() {
    let dir = tempfile::tempdir().expect("dir");
    local::write(dir.path(), "src/lib.rs", b"hello").expect("write");
    assert_eq!(
        local::read(dir.path(), "src/lib.rs").expect("read"),
        b"hello"
    );
    local::write(dir.path(), "deep/nested/file.txt", b"x").expect("write deep");
    let files = local::list(dir.path()).expect("list");
    assert!(files.contains(&"src/lib.rs".to_owned()));
    assert!(files.contains(&"deep/nested/file.txt".to_owned()));
}

#[tokio::test]
async fn git_fork_starts_at_parent_base_revision() {
    let parent = tempfile::tempdir().expect("parent");
    let rev1 = init_repo(&parent);
    let (store, epoch, _d) = open().await;
    let mut tx = txn(&store, epoch).await;
    let ids = DeterministicIds::new(SEED + 2);
    let parent_id = WorkspaceId::new(&ids);
    let child_id = WorkspaceId::new(&ids);
    local::record_workspace(&mut *tx, parent_id, parent.path(), 1)
        .await
        .expect("record parent");
    // HEAD moves on after the workspace base was recorded — the fork must
    // still land on the recorded revision.
    advance_head(&parent);
    let child_dir = tempfile::tempdir().expect("child root");
    let child_path = child_dir.path().join("fork");
    let caps = local::fork(&mut *tx, parent.path(), parent_id, child_id, &child_path, 2)
        .await
        .expect("fork");
    assert!(caps.git_semantic);
    tx.commit().await.expect("commit");

    // The fork is pinned at the recorded base, not floating HEAD.
    assert_eq!(
        local::base_revision(&child_path).expect("rev"),
        Some(rev1.clone())
    );
    assert_eq!(
        std::fs::read_to_string(child_path.join("a.txt")).expect("file"),
        "rev1"
    );

    let mut tx = txn(&store, epoch).await;
    let row = tx
        .workspaces()
        .get_workspace(child_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.kind, "git-worktree");
    assert_eq!(row.base_revision.as_deref(), Some(rev1.as_str()));
    assert_eq!(row.parent_workspace_id, Some(parent_id));

    local::remove(parent.path(), &child_path).expect("remove worktree");
}

#[tokio::test]
async fn non_git_fork_reports_copy_capability() {
    let parent = tempfile::tempdir().expect("parent");
    std::fs::write(parent.path().join("x.txt"), "data").expect("write");
    let (store, epoch, _d) = open().await;
    let mut tx = txn(&store, epoch).await;
    let ids = DeterministicIds::new(SEED + 3);
    let parent_id = WorkspaceId::new(&ids);
    let child_id = WorkspaceId::new(&ids);
    local::record_workspace(&mut *tx, parent_id, parent.path(), 1)
        .await
        .expect("record");
    let child_dir = tempfile::tempdir().expect("child");
    let child_path = child_dir.path().join("fork");
    let caps = local::fork(&mut *tx, parent.path(), parent_id, child_id, &child_path, 2)
        .await
        .expect("fork");
    // Honest capability label — a copy fork is not Git-semantic.
    assert!(!caps.git_semantic);
    assert_eq!(caps.kind, "copy-fork");
    assert!(!caps.is_security_boundary);
    tx.commit().await.expect("commit");
    assert_eq!(
        std::fs::read_to_string(child_path.join("x.txt")).expect("copied"),
        "data"
    );
    let mut tx = txn(&store, epoch).await;
    let row = tx
        .workspaces()
        .get_workspace(child_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row.kind, "copy-fork");
    assert!(row.base_revision.is_none());
}

#[test]
fn path_traversal_denied() {
    let dir = tempfile::tempdir().expect("dir");
    for bad in ["../etc/passwd", "a/../../b", "/abs", "a\\b"] {
        assert!(
            local::resolve_relative(dir.path(), bad).is_err(),
            "{bad} must be denied"
        );
    }
    // A symlink inside the workspace pointing outside must be denied on
    // canonicalize.
    std::fs::write(dir.path().join("real.txt"), "x").expect("file");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("secret"), "s").expect("secret");
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link"))
        .expect("symlink");
    assert!(local::resolve_relative(dir.path(), "link").is_err());
    assert!(local::resolve_relative(dir.path(), "new/nested.txt").is_ok());
}
