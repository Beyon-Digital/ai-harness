//! Atomicity of SQLite write transactions: rejection at admission, faulted
//! transactions, explicit rollback, and drop without commit leave zero rows.

use std::path::Path;

use domain::faults::FaultInjector;
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId,
    SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewSession, NewTask};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";

fn config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 8,
        busy_timeout_ms: 5_000,
    }
}

fn context(provider: &DeterministicIds, daemon_epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch,
        principal_id: PrincipalId::new(provider),
        command_id: CommandId::new(provider),
        correlation_id: None,
    }
}

async fn open_store(dir: &Path) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(config(&dir.join(DB_FILE)))
        .await
        .unwrap();
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0)
}

async fn insert_session(
    txn: &mut Box<dyn KernelTxn + '_>,
    provider: &DeterministicIds,
    session: SessionId,
) {
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(provider),
            created_at_ms: 10,
            metadata: Some(vec![7]),
        })
        .await
        .unwrap();
}

async fn insert_task(
    txn: &mut Box<dyn KernelTxn + '_>,
    provider: &DeterministicIds,
    task: TaskId,
    session: SessionId,
) {
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: ActorId::new(provider),
            task_kind: "test".to_owned(),
            payload: vec![1],
            created_at_ms: 10,
        })
        .await
        .unwrap();
}

async fn insert_run(
    txn: &mut Box<dyn KernelTxn + '_>,
    run: RunId,
    task: TaskId,
    session: SessionId,
) {
    txn.runs()
        .insert(NewRun {
            run_id: run,
            task_id: task,
            session_id: Some(session),
            parent_run_id: None,
            state: RunState::Created,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: cursor(run),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();
}

async fn session_is_absent(store: &SqliteKernelStore, session: SessionId) -> bool {
    let mut read = store.begin_read().await.unwrap();
    read.sessions().get(session).await.unwrap().is_none()
}

async fn rejected_write(store: &SqliteKernelStore, ctx: TxContext) -> errors::KernelError {
    match store.begin_write(ctx).await {
        Ok(_) => panic!("write transaction must be rejected"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn missing_or_stale_epoch_rejects_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);

    let missing = rejected_write(&store, context(&provider, 0)).await;
    assert_eq!(missing.code(), ErrorCode::FailedPrecondition);
    assert_eq!(missing.retry_class(), RetryClass::Never);

    let stale = rejected_write(&store, context(&provider, epoch + 1)).await;
    assert_eq!(stale.code(), ErrorCode::FailedPrecondition);
    assert_eq!(stale.retry_class(), RetryClass::Never);

    let session = SessionId::new(&provider);
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut txn, &provider, session).await;
    txn.commit().await.unwrap();
    assert!(!session_is_absent(&store, session).await);
}

#[tokio::test]
async fn fault_after_successful_write_leaves_zero_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let faults = ArmedFaults::new();
    let session = SessionId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut txn, &provider, session).await;
    faults.arm("store.write.aborted");
    faults.trigger("store.write.aborted");
    faults.assert_triggered("store.write.aborted");
    drop(txn);

    assert!(session_is_absent(&store, session).await);
}

#[tokio::test]
async fn drop_without_commit_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let session = SessionId::new(&provider);
    let task = TaskId::new(&provider);
    let run = RunId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut txn, &provider, session).await;
    insert_task(&mut txn, &provider, task, session).await;
    insert_run(&mut txn, run, task, session).await;
    drop(txn);

    assert!(session_is_absent(&store, session).await);
    let mut read = store.begin_read().await.unwrap();
    assert!(read.tasks().get(task).await.unwrap().is_none());
    assert!(read.runs().get(run).await.unwrap().is_none());
    drop(read);

    let mut next = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut next, &provider, session).await;
    next.commit().await.unwrap();
    assert!(!session_is_absent(&store, session).await);
}

#[tokio::test]
async fn explicit_rollback_discards_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let session = SessionId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut txn, &provider, session).await;
    txn.rollback().await.unwrap();

    assert!(session_is_absent(&store, session).await);
}

#[tokio::test]
async fn empty_commit_is_a_noop_success() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);

    let txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.commit().await.unwrap();
}

#[tokio::test]
async fn dropped_write_returns_a_clean_single_pooled_connection() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    let store = SqliteKernelStore::open(StoreConfig {
        path: db_path,
        pool_max_connections: 1,
        busy_timeout_ms: 5_000,
    })
    .await
    .unwrap();
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let epoch = fence.epoch.0;
    let session = SessionId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    insert_session(&mut txn, &provider, session).await;
    drop(txn);

    let mut next = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert!(
        next.sessions().get(session).await.unwrap().is_none(),
        "the queued rollback must run before the connection is reused"
    );
    next.commit().await.unwrap();
}
