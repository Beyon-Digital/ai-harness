//! SBOX-001: T0 exec, deadline/cancel, env allowlist, T2 fail-closed,
//! and the workspace-lease gate on coordinator-mediated exec.

use std::time::Duration;

use adapter_registry::capabilities::SandboxTier;
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId, TaskId,
    WorkspaceId,
};
use domain::resource::WorkspaceAccessMode;
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::ErrorCode;
use kernel_store::models::{NewRun, NewTask};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sandbox::local_process::{ExecSpec, ExecStatus};
use sandbox::{exec_in_workspace, resolve_sandbox};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;
use workspace::{coordinator, local};

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;

fn spec(cwd: &std::path::Path, program: &str, args: &[&str], deadline_ms: u64) -> ExecSpec {
    ExecSpec {
        program: program.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
        cwd: cwd.to_path_buf(),
        env: vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), cwd.to_string_lossy().into_owned()),
        ],
        deadline: Duration::from_millis(deadline_ms),
        output_cap_bytes: 64 * 1024,
    }
}

fn no_cancel() -> tokio::sync::watch::Receiver<bool> {
    tokio::sync::watch::channel(false).1
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

async fn run_row(txn: &mut dyn KernelTxn, run_id: RunId, ids: &DeterministicIds) {
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
            input_event_cursor: EventCursor::new(
                EventStreamKey::new(format!("run/{run_id}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: String::new(),
            workspace_uri: None,
            created_at_ms: NOW,
        })
        .await
        .expect("run");
}

#[tokio::test]
async fn t0_executes_and_captures() {
    let dir = tempfile::tempdir().expect("dir");
    let out = sandbox::local_process::exec(
        &spec(
            dir.path(),
            "/bin/sh",
            &["-c", "echo hello; echo err >&2"],
            5_000,
        ),
        no_cancel(),
    )
    .await
    .expect("exec");
    assert_eq!(out.status, ExecStatus::Exited(0));
    assert_eq!(out.stdout, b"hello\n");
    assert_eq!(out.stderr, b"err\n");
    let instance = resolve_sandbox(SandboxTier::T0, &[]).expect("t0 resolves");
    assert!(
        !instance.is_security_boundary,
        "T0 is not a security boundary"
    );
}

#[tokio::test]
async fn deadline_and_cancel_terminate() {
    let dir = tempfile::tempdir().expect("dir");
    let out = sandbox::local_process::exec(
        &spec(dir.path(), "/bin/sh", &["-c", "sleep 30"], 100),
        no_cancel(),
    )
    .await
    .expect("exec");
    assert_eq!(out.status, ExecStatus::TimedOut);
    assert!(out.duration < Duration::from_secs(10));

    let (tx, rx) = tokio::sync::watch::channel(false);
    let dir2 = tempfile::tempdir().expect("dir2");
    let s = spec(dir2.path(), "/bin/sh", &["-c", "sleep 30"], 60_000);
    let handle = tokio::spawn(async move { sandbox::local_process::exec(&s, rx).await });
    tokio::task::yield_now().await;
    tx.send(true).expect("cancel");
    let out = handle.await.expect("join").expect("exec");
    assert_eq!(out.status, ExecStatus::Cancelled);
}

#[tokio::test]
async fn env_allowlist_is_exact() {
    let dir = tempfile::tempdir().expect("dir");
    // SAFETY note: only the allowlisted vars reach the child — HOME is
    // set, and a non-allowlisted parent var must be absent.
    unsafe { std::env::set_var("SANDBOX_TEST_SECRET", "parent-value") };
    let mut s = spec(
        dir.path(),
        "/bin/sh",
        &["-c", "echo \"$SANDBOX_TEST_SECRET|$MARKER\""],
        5_000,
    );
    s.env.push(("MARKER".into(), "approved".into()));
    let out = sandbox::local_process::exec(&s, no_cancel())
        .await
        .expect("exec");
    assert_eq!(out.stdout, b"|approved\n");
    unsafe { std::env::remove_var("SANDBOX_TEST_SECRET") };
}

#[tokio::test]
async fn t2_fails_closed() {
    for tier in [SandboxTier::T1, SandboxTier::T2, SandboxTier::T3] {
        let err = resolve_sandbox(tier, &[]).expect_err("no adapter => no tier");
        assert_eq!(err.code(), ErrorCode::FailedPrecondition);
        assert!(err.message().contains("capability unsupported"));
    }
}

#[tokio::test]
async fn revoked_lease_blocks_coordinator_exec() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 9);
    let ws = WorkspaceId::new(&ids);
    let run = RunId::new(&ids);
    let root = tempfile::tempdir().expect("root");

    let mut tx = txn(&store, epoch).await;
    local::record_workspace(&mut *tx, ws, root.path(), NOW)
        .await
        .expect("ws");
    run_row(&mut *tx, run, &ids).await;
    let lease = coordinator::acquire_lease(
        &mut *tx,
        &ids,
        ws,
        run,
        WorkspaceAccessMode::ExclusiveWrite,
        &[],
        Vec::new(),
        NOW,
    )
    .await
    .expect("lease");
    tx.commit().await.expect("commit");

    // Live lease: exec works.
    let mut tx = txn(&store, epoch).await;
    let out = exec_in_workspace(
        &mut *tx,
        &spec(root.path(), "/bin/sh", &["-c", "echo ok"], 5_000),
        ws,
        lease.lease_id,
        run,
        lease.lease_epoch,
        no_cancel(),
    )
    .await
    .expect("exec under live lease");
    assert_eq!(out.stdout, b"ok\n");

    // Revoke, then the same token must not exec.
    coordinator::revoke_lease(&mut *tx, lease.lease_id, lease.lease_epoch)
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let err = exec_in_workspace(
        &mut *tx,
        &spec(
            root.path(),
            "/bin/sh",
            &["-c", "echo should-not-run"],
            5_000,
        ),
        ws,
        lease.lease_id,
        run,
        lease.lease_epoch,
        no_cancel(),
    )
    .await
    .expect_err("revoked lease blocks exec");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}
