//! Writer contention: `BEGIN IMMEDIATE` serializes racing CAS updates, and a
//! deliberately tiny busy timeout surfaces as retry-safe `Unavailable`
//! (R3.2, R3.3, R3.4, N2).

use std::path::Path;
use std::sync::Arc;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, RunId,
    SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRun, NewSession, NewTask, RunCas, RunPatch};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const DB_FILE: &str = "kernel.db";
const WRITERS: usize = 6;

fn config(db_path: &Path, pool_max_connections: u32, busy_timeout_ms: u64) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections,
        busy_timeout_ms,
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

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0)
}

async fn seed_run(store: &SqliteKernelStore, provider: &DeterministicIds, epoch: u64) -> RunId {
    let session = SessionId::new(provider);
    let task = TaskId::new(provider);
    let run = RunId::new(provider);
    let mut txn = store.begin_write(context(provider, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: ActorId::new(provider),
            task_kind: "test".to_owned(),
            payload: vec![],
            created_at_ms: 10,
        })
        .await
        .unwrap();
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
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: String::new(),
            workspace_uri: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_writer_wins_a_raced_cas() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteKernelStore::open(config(&dir.path().join(DB_FILE), 8, 5_000))
            .await
            .unwrap(),
    );
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let epoch = fence.epoch.0;
    let run = seed_run(&store, &provider, epoch).await;

    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut handles = Vec::new();
    for _ in 0..WRITERS {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let ctx = context(&provider, epoch);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut txn = store.begin_write(ctx).await.unwrap();
            let won = txn
                .runs()
                .cas_update(
                    run,
                    RunCas {
                        run_revision: 0,
                        state: None,
                        cancellation_epoch: None,
                    },
                    RunPatch {
                        step_sequence: Some(1),
                        bump_revision: true,
                        ..RunPatch::default()
                    },
                )
                .await
                .unwrap();
            txn.commit().await.unwrap();
            won
        }));
    }
    let mut wins = 0;
    for handle in handles {
        if handle.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "exactly one racing writer may win the CAS");

    let mut read = store.begin_read().await.unwrap();
    let row = read.runs().get(run).await.unwrap().unwrap();
    assert_eq!(row.run_revision, 1);
    assert_eq!(row.step_sequence, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contended_begin_write_maps_busy_to_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteKernelStore::open(config(&dir.path().join(DB_FILE), 5, 1))
        .await
        .unwrap();
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let epoch = fence.epoch.0;

    let mut holder = store.begin_write(context(&provider, epoch)).await.unwrap();
    holder
        .sessions()
        .insert(NewSession {
            session_id: SessionId::new(&provider),
            principal_id: PrincipalId::new(&provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();

    let error = match store.begin_write(context(&provider, epoch)).await {
        Ok(_) => panic!("a second writer must not begin while the lock is held"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::Unavailable);
    assert_eq!(error.retry_class(), RetryClass::Safe);

    holder.commit().await.unwrap();
    let next = store.begin_write(context(&provider, epoch)).await.unwrap();
    next.commit().await.unwrap();
}
