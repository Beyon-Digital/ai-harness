//! Transactional outbox: contiguous in-transaction allocation, unique event
//! identities and stream positions, ordered unpublished scans, publication
//! metadata updates, immutability, and commit/rollback visibility
//! (R6.1-R6.3, R6.5).

use std::path::Path;

use domain::ids::{CommandId, DaemonInstanceId, EventId, EventStreamKey, PrincipalId};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::NewOutboxEvent;
use kernel_store::repositories::PublishKind;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sqlx::Sqlite;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";
const SEED_MS: i64 = 1_700_000_000_000;

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
    let provider = DeterministicIds::new(SEED_MS);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

async fn direct_connection(db_path: &Path) -> PoolConnection<Sqlite> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
        .acquire()
        .await
        .unwrap()
}

async fn direct_count(db_path: &Path) -> i64 {
    let mut connection = direct_connection(db_path).await;
    sqlx::query_scalar("SELECT COUNT(*) FROM outbox_events")
        .fetch_one(&mut *connection)
        .await
        .unwrap()
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
) -> NewOutboxEvent {
    NewOutboxEvent {
        event_id: EventId::new(provider),
        event_type: "run.created".to_owned(),
        event_version: 1,
        stream_key: stream_key.clone(),
        sequence,
        occurred_at_ms: SEED_MS,
        run_id: None,
        task_id: None,
        session_id: None,
        effect_id: None,
        causation_id: None,
        correlation_id: Some("corr-1".to_owned()),
        sensitivity: SensitivityClass::Internal,
        retention: RetentionClass::Audit,
        payload: vec![0x07],
    }
}

#[tokio::test]
async fn allocate_returns_contiguous_sequences_per_stream() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let left = stream("run/left");
    let right = stream("run/right");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert_eq!(txn.streams().allocate(left.clone()).await.unwrap(), 1);
    assert_eq!(txn.streams().allocate(right.clone()).await.unwrap(), 1);
    assert_eq!(txn.streams().allocate(left.clone()).await.unwrap(), 2);
    assert_eq!(txn.streams().allocate(left.clone()).await.unwrap(), 3);
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert_eq!(txn.streams().allocate(left).await.unwrap(), 4);
    assert_eq!(txn.streams().allocate(right).await.unwrap(), 2);
    txn.commit().await.unwrap();
}

#[tokio::test]
async fn duplicate_event_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let stream_key = stream("run/duplicate");

    let first = event(&provider, &stream_key, 1);
    let duplicate_id = first.event_id;
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.streams().insert_outbox(first).await.unwrap();
    txn.commit().await.unwrap();

    let mut second = event(&provider, &stream_key, 2);
    second.event_id = duplicate_id;
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let error = txn
        .streams()
        .insert_outbox(second)
        .await
        .expect_err("a duplicate event id must conflict");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    txn.rollback().await.unwrap();
    assert_eq!(direct_count(&dir.path().join(DB_FILE)).await, 1);
}

#[tokio::test]
async fn duplicate_stream_sequence_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let stream_key = stream("run/position");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.streams()
        .insert_outbox(event(&provider, &stream_key, 1))
        .await
        .unwrap();
    let error = txn
        .streams()
        .insert_outbox(event(&provider, &stream_key, 1))
        .await
        .expect_err("a duplicate stream position must conflict");
    assert_eq!(error.code(), ErrorCode::Conflict);
    txn.rollback().await.unwrap();
    assert_eq!(direct_count(&dir.path().join(DB_FILE)).await, 0);
}

#[tokio::test]
async fn scan_unpublished_is_ordered_and_honors_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let alpha = stream("run/alpha");
    let beta = stream("run/beta");
    let gamma = stream("run/gamma");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let beta_one = event(&provider, &beta, 1);
    let alpha_two = event(&provider, &alpha, 2);
    let alpha_one = event(&provider, &alpha, 1);
    let gamma_one = event(&provider, &gamma, 1);
    for row in [&beta_one, &alpha_two, &alpha_one, &gamma_one] {
        txn.streams().insert_outbox(row.clone()).await.unwrap();
    }
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let scanned = txn.streams().scan_unpublished(10).await.unwrap();
    let ids: Vec<EventId> = scanned.iter().map(|row| row.event_id).collect();
    assert_eq!(
        ids,
        vec![
            alpha_one.event_id,
            alpha_two.event_id,
            beta_one.event_id,
            gamma_one.event_id
        ]
    );
    assert_eq!(scanned[0].sequence, 1);
    assert_eq!(scanned[1].sequence, 2);
    assert!(
        scanned
            .iter()
            .all(|row| row.journal_published_at_ms.is_none())
    );

    let limited = txn.streams().scan_unpublished(2).await.unwrap();
    let limited_ids: Vec<EventId> = limited.iter().map(|row| row.event_id).collect();
    assert_eq!(limited_ids, vec![alpha_one.event_id, alpha_two.event_id]);
    txn.rollback().await.unwrap();
}

#[tokio::test]
async fn mark_published_updates_only_the_selected_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let stream_key = stream("run/markers");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let journal = event(&provider, &stream_key, 1);
    let live = event(&provider, &stream_key, 2);
    txn.streams().insert_outbox(journal.clone()).await.unwrap();
    txn.streams().insert_outbox(live.clone()).await.unwrap();
    txn.streams()
        .mark_published(journal.event_id, PublishKind::Journal)
        .await
        .unwrap();
    txn.streams()
        .mark_published(live.event_id, PublishKind::Live)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let scanned = txn.streams().scan_unpublished(10).await.unwrap();
    assert_eq!(
        scanned.len(),
        1,
        "live publication keeps the journal marker null"
    );
    assert_eq!(scanned[0].event_id, live.event_id);
    assert!(scanned[0].live_published_at_ms.is_some());
    let not_found = txn
        .streams()
        .mark_published(EventId::new(&provider), PublishKind::Journal)
        .await
        .expect_err("publishing an absent event is not found");
    assert_eq!(not_found.code(), ErrorCode::NotFound);
    assert_eq!(not_found.retry_class(), RetryClass::Never);
    txn.streams()
        .mark_published(live.event_id, PublishKind::Journal)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut connection = direct_connection(&dir.path().join(DB_FILE)).await;
    let (journal_ms, live_ms): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT journal_published_at_ms, live_published_at_ms FROM outbox_events \
         WHERE event_id = ?1",
    )
    .bind(journal.event_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(journal_ms.is_some());
    assert!(live_ms.is_none());

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert!(txn.streams().scan_unpublished(10).await.unwrap().is_empty());
    txn.rollback().await.unwrap();
}

#[tokio::test]
async fn outbox_rows_reject_mutation_outside_publication_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let stream_key = stream("run/immutable");
    let staged = event(&provider, &stream_key, 1);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.streams().insert_outbox(staged.clone()).await.unwrap();
    txn.commit().await.unwrap();

    let mut connection = direct_connection(&dir.path().join(DB_FILE)).await;
    let error = sqlx::query("UPDATE outbox_events SET payload = ?1 WHERE event_id = ?2")
        .bind(vec![0x00u8])
        .bind(staged.event_id.to_string())
        .execute(&mut *connection)
        .await
        .expect_err("payload mutation must be rejected by the schema trigger");
    let code = error
        .as_database_error()
        .expect("constraint failure")
        .code();
    assert_eq!(code.as_deref(), Some("1811"));
    drop(connection);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let scanned = txn.streams().scan_unpublished(10).await.unwrap();
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].payload, staged.payload);
    txn.rollback().await.unwrap();
}

#[tokio::test]
async fn rollback_discards_outbox_rows_and_stream_allocations() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(SEED_MS);
    let stream_key = stream("run/atomic");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert_eq!(txn.streams().allocate(stream_key.clone()).await.unwrap(), 1);
    let committed = event(&provider, &stream_key, 1);
    txn.streams()
        .insert_outbox(committed.clone())
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut failing = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert_eq!(
        failing
            .streams()
            .allocate(stream_key.clone())
            .await
            .unwrap(),
        2
    );
    failing
        .streams()
        .insert_outbox(event(&provider, &stream_key, 2))
        .await
        .unwrap();
    assert_eq!(direct_count(&dir.path().join(DB_FILE)).await, 1);
    failing.rollback().await.unwrap();
    assert_eq!(direct_count(&dir.path().join(DB_FILE)).await, 1);

    let mut retry = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert_eq!(
        retry.streams().allocate(stream_key.clone()).await.unwrap(),
        2,
        "a rolled back allocation is not consumed"
    );
    let replacement = event(&provider, &stream_key, 2);
    retry
        .streams()
        .insert_outbox(replacement.clone())
        .await
        .unwrap();
    retry.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let scanned = txn.streams().scan_unpublished(10).await.unwrap();
    let sequences: Vec<u64> = scanned.iter().map(|row| row.sequence).collect();
    assert_eq!(sequences, vec![1, 2]);
    assert_eq!(scanned[1].event_id, replacement.event_id);
    txn.rollback().await.unwrap();
}
