//! Idempotency records: transactional replay lookup, same-digest replay,
//! digest conflicts, and exactly one insert under concurrent identical
//! submissions (R5.1-R5.5, P4).

use std::path::Path;
use std::sync::Arc;

use domain::ids::{CommandId, DaemonInstanceId, IdempotencyKey, PrincipalId};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::NewIdempotencyRecord;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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

fn key(text: &str) -> IdempotencyKey {
    match IdempotencyKey::new(text) {
        Ok(key) => key,
        Err(error) => panic!("idempotency key rejected: {error}"),
    }
}

fn record(
    provider: &DeterministicIds,
    principal: PrincipalId,
    idempotency_key: &IdempotencyKey,
    digest: &str,
    outcome_code: &str,
) -> NewIdempotencyRecord {
    NewIdempotencyRecord {
        principal_id: principal,
        idempotency_key: idempotency_key.clone(),
        request_digest: digest.to_owned(),
        command_id: CommandId::new(provider),
        outcome_code: outcome_code.to_owned(),
        outcome_payload: vec![0x01, 0x02, 0x03],
        created_at_ms: SEED_MS,
    }
}

async fn row_count(db_path: &Path) -> i64 {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM idempotency_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    count
}

#[tokio::test]
async fn insert_then_lookup_returns_the_stored_record() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), 4).await;
    let provider = DeterministicIds::new(SEED_MS);
    let principal = PrincipalId::new(&provider);
    let idempotency_key = key("command-1");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.idempotency()
        .insert(record(
            &provider,
            principal,
            &idempotency_key,
            "digest-1",
            "ok",
        ))
        .await
        .unwrap();
    let stored = txn
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("record visible inside its own transaction");
    txn.commit().await.unwrap();

    assert_eq!(stored.principal_id, principal);
    assert_eq!(stored.idempotency_key, idempotency_key);
    assert_eq!(stored.request_digest, "digest-1");
    assert_eq!(stored.outcome_code, "ok");
    assert_eq!(stored.outcome_payload, vec![0x01, 0x02, 0x03]);
    assert_eq!(stored.created_at_ms, SEED_MS);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let replayed = txn
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("committed record visible");
    txn.commit().await.unwrap();
    assert_eq!(replayed, stored);
}

#[tokio::test]
async fn lookup_inside_a_rolled_back_transaction_leaves_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), 4).await;
    let provider = DeterministicIds::new(SEED_MS);
    let principal = PrincipalId::new(&provider);
    let idempotency_key = key("command-2");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.idempotency()
        .insert(record(
            &provider,
            principal,
            &idempotency_key,
            "digest-1",
            "ok",
        ))
        .await
        .unwrap();
    assert!(
        txn.idempotency()
            .lookup(principal, &idempotency_key)
            .await
            .unwrap()
            .is_some(),
        "record visible before rollback"
    );
    txn.rollback().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert!(
        txn.idempotency()
            .lookup(principal, &idempotency_key)
            .await
            .unwrap()
            .is_none()
    );
    txn.commit().await.unwrap();
    assert_eq!(row_count(&dir.path().join(DB_FILE)).await, 0);
}

#[tokio::test]
async fn replaying_the_same_digest_returns_the_unchanged_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), 4).await;
    let provider = DeterministicIds::new(SEED_MS);
    let principal = PrincipalId::new(&provider);
    let idempotency_key = key("command-3");
    let original = record(
        &provider,
        principal,
        &idempotency_key,
        "digest-1",
        "created",
    );

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.idempotency().insert(original.clone()).await.unwrap();
    txn.commit().await.unwrap();

    let mut replay = store.begin_write(context(&provider, epoch)).await.unwrap();
    let stored = replay
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("a replay finds the stored record");
    assert_eq!(stored.request_digest, original.request_digest);
    assert_eq!(stored.outcome_code, original.outcome_code);
    assert_eq!(stored.outcome_payload, original.outcome_payload);
    assert_eq!(stored.created_at_ms, original.created_at_ms);
    replay.commit().await.unwrap();

    assert_eq!(
        row_count(&dir.path().join(DB_FILE)).await,
        1,
        "the replay must not insert a second row"
    );

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let error = txn
        .idempotency()
        .insert(original.clone())
        .await
        .expect_err("a blind duplicate insert is rejected by the unique key");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    let stored = txn
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("the conflicting insert leaves the stored record untouched");
    txn.rollback().await.unwrap();

    assert_eq!(stored.request_digest, original.request_digest);
    assert_eq!(stored.outcome_code, original.outcome_code);
    assert_eq!(stored.outcome_payload, original.outcome_payload);
    assert_eq!(stored.command_id, original.command_id);
    assert_eq!(stored.created_at_ms, original.created_at_ms);
    assert_eq!(row_count(&dir.path().join(DB_FILE)).await, 1);
}

#[tokio::test]
async fn same_key_with_a_different_digest_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), 4).await;
    let provider = DeterministicIds::new(SEED_MS);
    let principal = PrincipalId::new(&provider);
    let idempotency_key = key("command-4");

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.idempotency()
        .insert(record(
            &provider,
            principal,
            &idempotency_key,
            "digest-1",
            "ok",
        ))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let error = txn
        .idempotency()
        .insert(record(
            &provider,
            principal,
            &idempotency_key,
            "digest-2",
            "ok",
        ))
        .await
        .expect_err("a different digest must conflict");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    let stored = txn
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("original record survives the conflict");
    txn.commit().await.unwrap();

    assert_eq!(stored.request_digest, "digest-1");
    assert_eq!(row_count(&dir.path().join(DB_FILE)).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_submissions_produce_one_row() {
    const SUBMISSIONS: usize = 6;

    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path(), SUBMISSIONS as u32 + 2).await;
    let store = Arc::new(store);
    let provider = Arc::new(DeterministicIds::new(SEED_MS));
    let principal = PrincipalId::new(provider.as_ref());
    let idempotency_key = key("command-5");

    let barrier = Arc::new(Barrier::new(SUBMISSIONS));
    let mut handles = Vec::new();
    for _ in 0..SUBMISSIONS {
        let store = Arc::clone(&store);
        let provider = Arc::clone(&provider);
        let barrier = Arc::clone(&barrier);
        let idempotency_key = idempotency_key.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let ctx = TxContext {
                daemon_epoch: epoch,
                principal_id: PrincipalId::new(provider.as_ref()),
                command_id: CommandId::new(provider.as_ref()),
                correlation_id: None,
            };
            let mut txn = store.begin_write(ctx).await.unwrap();
            let replayed = txn
                .idempotency()
                .lookup(principal, &idempotency_key)
                .await
                .unwrap()
                .is_some();
            if !replayed {
                txn.idempotency()
                    .insert(record(
                        provider.as_ref(),
                        principal,
                        &idempotency_key,
                        "digest-1",
                        "ok",
                    ))
                    .await
                    .unwrap();
            }
            txn.commit().await.unwrap();
            replayed
        }));
    }

    let mut inserts = 0;
    for handle in handles {
        if !handle.await.unwrap() {
            inserts += 1;
        }
    }
    assert_eq!(inserts, 1, "exactly one racing submission may insert");
    assert_eq!(row_count(&dir.path().join(DB_FILE)).await, 1);

    let mut txn = store
        .begin_write(context(provider.as_ref(), epoch))
        .await
        .unwrap();
    let stored = txn
        .idempotency()
        .lookup(principal, &idempotency_key)
        .await
        .unwrap()
        .expect("winning record");
    txn.commit().await.unwrap();
    assert_eq!(stored.request_digest, "digest-1");
}
