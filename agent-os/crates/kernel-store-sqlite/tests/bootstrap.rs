//! Bootstrap acceptance tests for the versioned kernel database.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use errors::codes::{ErrorCode, RetryClass};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

const DB_FILE: &str = "kernel.db";
const EXPECTED_TABLE_COUNT: usize = 30;

fn config_for(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 5,
        busy_timeout_ms: 5_000,
    }
}

async fn direct_pool(db_path: &Path, foreign_keys: bool) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false)
        .foreign_keys(foreign_keys);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap()
}

async fn schema_snapshot(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT type, name FROM sqlite_master ORDER BY type, name")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn read_version(pool: &SqlitePool) -> String {
    sqlx::query_scalar("SELECT value FROM kernel_meta WHERE key = 'schema_version'")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn assert_mode(path: &Path, expected: u32) {
    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, expected, "mode of {}", path.display());
}

fn set_mode(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

#[tokio::test]
async fn fresh_bootstrap_creates_database_from_schema() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);

    let store = SqliteKernelStore::open(config_for(&db_path)).await.unwrap();

    assert_eq!(store.path(), db_path.as_path());
    assert!(db_path.is_file());
    assert_mode(&db_path, 0o600);
    assert_mode(dir.path(), 0o700);

    let pool = direct_pool(&db_path, false).await;
    assert_eq!(read_version(&pool).await, "1");

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tables as usize, EXPECTED_TABLE_COUNT);

    for known in ["kernel_meta", "runs", "outbox_events", "daemon_fence"] {
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(known)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(found.as_deref(), Some(known));
    }

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

#[tokio::test]
async fn reopen_is_structurally_stable() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    let pool = direct_pool(&db_path, false).await;
    let before = schema_snapshot(&pool).await;
    sqlx::raw_sql("DROP INDEX idx_runs_task")
        .execute(&pool)
        .await
        .unwrap();
    let after_drop = schema_snapshot(&pool).await;
    drop(pool);
    assert!(before.len() > after_drop.len());

    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    let pool = direct_pool(&db_path, false).await;
    let after_reopen = schema_snapshot(&pool).await;
    assert_eq!(
        after_drop, after_reopen,
        "reopen altered the database schema"
    );
    assert!(
        !after_reopen.iter().any(|(_, name)| name == "idx_runs_task"),
        "reopen re-executed the schema and re-created a dropped index"
    );
    assert_eq!(read_version(&pool).await, "1");
}

#[tokio::test]
async fn wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    let pool = direct_pool(&db_path, false).await;
    sqlx::query("UPDATE kernel_meta SET value = '2' WHERE key = 'schema_version'")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);

    let error = SqliteKernelStore::open(config_for(&db_path))
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        error.message().contains('2'),
        "error must name the version found: {}",
        error.message()
    );

    let pool = direct_pool(&db_path, false).await;
    assert_eq!(
        read_version(&pool).await,
        "2",
        "a failed open must not rewrite the version"
    );
}

#[tokio::test]
async fn missing_schema_version_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    let pool = direct_pool(&db_path, false).await;
    sqlx::query("DELETE FROM kernel_meta WHERE key = 'schema_version'")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);

    let error = SqliteKernelStore::open(config_for(&db_path))
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        error.message().contains("missing"),
        "error must name the absent version: {}",
        error.message()
    );

    let pool = direct_pool(&db_path, false).await;
    let seeded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kernel_meta WHERE key = 'schema_version'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seeded, 0, "a failed open must not seed the version");
}

#[tokio::test]
async fn schema_constraints_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    let pool = direct_pool(&db_path, true).await;
    sqlx::query(
        "INSERT INTO tasks (task_id, session_id, created_by_actor_id, task_kind, payload, created_at_ms) \
         VALUES ('task-1', NULL, 'actor-1', 'kind-1', X'00', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let check_error = sqlx::query(
        "INSERT INTO runs (run_id, task_id, state, recovery_disposition, created_at_ms, updated_at_ms) \
         VALUES ('run-bad-state', 'task-1', 99, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        check_error.to_string().contains("CHECK constraint failed"),
        "unexpected error: {check_error}"
    );

    let unique_error =
        sqlx::query("INSERT INTO kernel_meta (key, value) VALUES ('schema_version', '1')")
            .execute(&pool)
            .await
            .unwrap_err();
    assert!(
        unique_error
            .to_string()
            .contains("UNIQUE constraint failed"),
        "unexpected error: {unique_error}"
    );

    let fk_error = sqlx::query(
        "INSERT INTO runs (run_id, task_id, state, recovery_disposition, created_at_ms, updated_at_ms) \
         VALUES ('run-bad-fk', 'missing-task', 1, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        fk_error
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "unexpected error: {fk_error}"
    );

    sqlx::query(
        "INSERT INTO runs (run_id, task_id, state, recovery_disposition, created_at_ms, updated_at_ms) \
         VALUES ('run-1', 'task-1', 1, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn restrictive_modes_are_enforced_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    set_mode(dir.path(), 0o755);
    set_mode(&db_path, 0o644);

    drop(SqliteKernelStore::open(config_for(&db_path)).await.unwrap());

    assert_mode(dir.path(), 0o700);
    assert_mode(&db_path, 0o600);
}

#[tokio::test]
async fn missing_runtime_directory_is_created_0700() {
    let outer = tempfile::tempdir().unwrap();
    let runtime_dir = outer.path().join("nested/runtime");
    let db_path = runtime_dir.join(DB_FILE);

    let store = SqliteKernelStore::open(config_for(&db_path)).await.unwrap();

    assert_eq!(store.path(), db_path.as_path());
    assert!(runtime_dir.is_dir());
    assert_mode(&runtime_dir, 0o700);
    assert_mode(&db_path, 0o600);
}

#[tokio::test]
async fn path_without_parent_directory_is_rejected() {
    let error = SqliteKernelStore::open(StoreConfig {
        path: PathBuf::from(DB_FILE),
        pool_max_connections: 5,
        busy_timeout_ms: 5_000,
    })
    .await
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}
