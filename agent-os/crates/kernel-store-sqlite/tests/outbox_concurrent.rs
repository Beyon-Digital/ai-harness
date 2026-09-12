//! Concurrent stream appends: barrier-synchronized tasks stage bursts on one
//! stream, and the allocations are unique and contiguous (R6.4, P3, N2).
//!
//! Every write transaction reserves writer ordering with `BEGIN IMMEDIATE`, so
//! allocations are serialized even though `RETURNING` computes each next value
//! from the persisted head. The assertions are about the persisted result, not
//! about task scheduling: any barrier interleaving must yield `1..=total`.

use std::path::Path;
use std::sync::Arc;

use domain::ids::{CommandId, DaemonInstanceId, EventId, EventStreamKey, PrincipalId};
use domain::security::{RetentionClass, SensitivityClass};
use kernel_store::models::NewOutboxEvent;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const DB_FILE: &str = "kernel.db";
const SEED_MS: i64 = 1_700_000_000_000;

fn config(db_path: &Path, pool_max_connections: u32) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections,
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

async fn open_store(dir: &Path, pool_max_connections: u32) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(config(&dir.join(DB_FILE), pool_max_connections))
        .await
        .unwrap();
    let provider = DeterministicIds::new(SEED_MS);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

fn stream(key: &str) -> EventStreamKey {
    match EventStreamKey::new(key) {
        Ok(stream) => stream,
        Err(error) => panic!("stream key rejected: {error}"),
    }
}

fn event(
    provider: &DeterministicIds,
    stream_key: &EventStreamKey,
    sequence: u64,
    task: usize,
) -> NewOutboxEvent {
    NewOutboxEvent {
        event_id: EventId::new(provider),
        event_type: format!("burst.{task}"),
        event_version: 1,
        stream_key: stream_key.clone(),
        sequence,
        occurred_at_ms: SEED_MS,
        run_id: None,
        task_id: None,
        session_id: None,
        effect_id: None,
        causation_id: None,
        correlation_id: Some(format!("task-{task}")),
        sensitivity: SensitivityClass::Internal,
        retention: RetentionClass::Audit,
        payload: vec![task as u8],
    }
}

async fn assert_contiguous(
    store: &SqliteKernelStore,
    provider: &DeterministicIds,
    epoch: u64,
    total: u64,
) {
    let mut txn = store.begin_write(context(provider, epoch)).await.unwrap();
    let scanned = txn.streams().scan_unpublished(total as u32).await.unwrap();
    let sequences: Vec<u64> = scanned.iter().map(|row| row.sequence).collect();
    assert_eq!(
        sequences,
        (1..=total).collect::<Vec<u64>>(),
        "the scan must return the stream in contiguous sequence order"
    );
    txn.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn barrier_bursts_with_one_transaction_per_event_stay_contiguous() {
    const TASKS: usize = 6;
    const BURST: usize = 8;
    const TOTAL: u64 = (TASKS * BURST) as u64;

    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), TASKS as u32 + 2).await;
    let store = Arc::new(store);
    let provider = Arc::new(DeterministicIds::new(SEED_MS));
    let stream_key = stream("run/barrier-per-event");
    let barrier = Arc::new(Barrier::new(TASKS));

    let mut handles = Vec::new();
    for task in 0..TASKS {
        let store = Arc::clone(&store);
        let provider = Arc::clone(&provider);
        let barrier = Arc::clone(&barrier);
        let stream_key = stream_key.clone();
        handles.push(tokio::spawn(async move {
            let mut allocated = Vec::with_capacity(BURST);
            barrier.wait().await;
            for _ in 0..BURST {
                let mut txn = store
                    .begin_write(context(provider.as_ref(), epoch))
                    .await
                    .unwrap();
                let sequence = txn.streams().allocate(stream_key.clone()).await.unwrap();
                txn.streams()
                    .insert_outbox(event(provider.as_ref(), &stream_key, sequence, task))
                    .await
                    .unwrap();
                txn.commit().await.unwrap();
                allocated.push(sequence);
            }
            allocated
        }));
    }

    let mut allocated = Vec::with_capacity(TOTAL as usize);
    for handle in handles {
        allocated.extend(handle.await.unwrap());
    }
    allocated.sort_unstable();
    assert_eq!(
        allocated,
        (1..=TOTAL).collect::<Vec<u64>>(),
        "concurrent allocations must be unique and gap-free"
    );
    assert_contiguous(&store, &provider, epoch, TOTAL).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn barrier_bursts_staged_in_one_transaction_stay_contiguous() {
    const TASKS: usize = 4;
    const BURST: usize = 6;
    const TOTAL: u64 = (TASKS * BURST) as u64;

    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), TASKS as u32 + 2).await;
    let store = Arc::new(store);
    let provider = Arc::new(DeterministicIds::new(SEED_MS));
    let stream_key = stream("run/barrier-per-burst");
    let barrier = Arc::new(Barrier::new(TASKS));

    let mut handles = Vec::new();
    for task in 0..TASKS {
        let store = Arc::clone(&store);
        let provider = Arc::clone(&provider);
        let barrier = Arc::clone(&barrier);
        let stream_key = stream_key.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut txn = store
                .begin_write(context(provider.as_ref(), epoch))
                .await
                .unwrap();
            let mut allocated = Vec::with_capacity(BURST);
            for _ in 0..BURST {
                let sequence = txn.streams().allocate(stream_key.clone()).await.unwrap();
                txn.streams()
                    .insert_outbox(event(provider.as_ref(), &stream_key, sequence, task))
                    .await
                    .unwrap();
                allocated.push(sequence);
            }
            txn.commit().await.unwrap();
            allocated
        }));
    }

    let mut allocated = Vec::with_capacity(TOTAL as usize);
    for handle in handles {
        allocated.extend(handle.await.unwrap());
    }
    allocated.sort_unstable();
    assert_eq!(
        allocated,
        (1..=TOTAL).collect::<Vec<u64>>(),
        "a burst committed in one transaction must extend the stream without gaps"
    );
    assert_contiguous(&store, &provider, epoch, TOTAL).await;
}
