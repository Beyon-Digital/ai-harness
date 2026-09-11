//! Daemon fence acceptance tests: first claim, strict epoch growth across
//! successive claims and restarts, live leases, and stale-epoch rejection
//! without mutation (R4.2-R4.5, N2).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use domain::ids::{CommandId, DaemonInstanceId, PrincipalId, SessionId};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::NewSession;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";
/// `daemon.fence_lease_ms` in `agent-os-microkernel-mvp-buildpack/specs/limits.yaml`.
const NORMATIVE_FENCE_LEASE_MS: i64 = 15_000;

fn config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 5,
        busy_timeout_ms: 5_000,
    }
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(elapsed.as_millis()).unwrap()
}

fn context(provider: &DeterministicIds, daemon_epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch,
        principal_id: PrincipalId::new(provider),
        command_id: CommandId::new(provider),
        correlation_id: None,
    }
}

async fn open_store(dir: &Path) -> errors::Result<SqliteKernelStore> {
    SqliteKernelStore::open(config(&dir.join(DB_FILE))).await
}

async fn direct_pool(db_path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
}

async fn session_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn first_claim_creates_epoch_one_with_a_live_lease() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path()).await.unwrap();
    assert!(store.current_fence().await.unwrap().is_none());

    let provider = DeterministicIds::new(1_700_000_000_000);
    let before = now_ms();
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let after = now_ms();

    assert_eq!(fence.epoch.0, 1);
    assert_ne!(
        fence.lease_expires_unix_ms,
        i64::MAX,
        "placeholder lease survived the claim"
    );
    assert!(
        fence.lease_expires_unix_ms >= before + NORMATIVE_FENCE_LEASE_MS,
        "lease expires before the normative window elapses"
    );
    assert!(
        fence.lease_expires_unix_ms <= after + NORMATIVE_FENCE_LEASE_MS,
        "lease outlives the normative window"
    );
    assert_eq!(store.current_fence().await.unwrap(), Some(fence));
}

#[tokio::test]
async fn successive_claims_strictly_increase_the_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path()).await.unwrap();
    let provider = DeterministicIds::new(1_700_000_000_001);

    let first = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let second = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();

    assert_eq!(first.epoch.0, 1);
    assert_eq!(second.epoch.0, first.epoch.0 + 1);
    assert!(second.lease_expires_unix_ms >= first.lease_expires_unix_ms);
    assert_eq!(store.current_fence().await.unwrap(), Some(second));
}

#[tokio::test]
async fn restart_keeps_the_fence_and_increments_the_next_claim() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DeterministicIds::new(1_700_000_000_002);

    {
        let store = open_store(dir.path()).await.unwrap();
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(&provider))
            .await
            .unwrap();
        assert_eq!(fence.epoch.0, 1);
    }

    let store = open_store(dir.path()).await.unwrap();
    let persisted = store.current_fence().await.unwrap().unwrap();
    assert_eq!(persisted.epoch.0, 1);

    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    assert_eq!(fence.epoch.0, persisted.epoch.0 + 1);
}

#[tokio::test]
async fn concurrent_claims_serialize_into_distinct_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DeterministicIds::new(1_700_000_000_004);
    let first_store = open_store(dir.path()).await.unwrap();
    let second_store = open_store(dir.path()).await.unwrap();

    let first_claim = first_store.acquire_daemon_fence(DaemonInstanceId::new(&provider));
    let second_claim = second_store.acquire_daemon_fence(DaemonInstanceId::new(&provider));
    let (first, second) = tokio::join!(first_claim, second_claim);

    let mut epochs = [first.unwrap().epoch.0, second.unwrap().epoch.0];
    epochs.sort_unstable();
    assert_eq!(
        epochs,
        [1, 2],
        "concurrent claims must serialize into consecutive epochs"
    );
    assert_eq!(
        first_store.current_fence().await.unwrap().unwrap().epoch.0,
        2
    );
}

#[tokio::test]
async fn stale_epoch_transaction_is_rejected_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DeterministicIds::new(1_700_000_000_003);
    let store = open_store(dir.path()).await.unwrap();

    let first = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let mut txn = store
        .begin_write(context(&provider, first.epoch.0))
        .await
        .unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: SessionId::new(&provider),
            principal_id: PrincipalId::new(&provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let second = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    assert_eq!(second.epoch.0, first.epoch.0 + 1);

    let stale = store.begin_write(context(&provider, first.epoch.0)).await;
    let error = match stale {
        Ok(_) => panic!("stale epoch admitted a write transaction"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let pool = direct_pool(&dir.path().join(DB_FILE)).await;
    assert_eq!(
        session_count(&pool).await,
        1,
        "replaying the stale transaction mutated committed state"
    );
    let persisted: i64 =
        sqlx::query_scalar("SELECT fencing_epoch FROM daemon_fence WHERE singleton = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, i64::try_from(second.epoch.0).unwrap());
}
