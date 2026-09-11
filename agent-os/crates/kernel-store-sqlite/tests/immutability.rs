//! Storage-layer immutability for resolved run environments and bindings.
//!
//! The schema triggers reject updates and deletes with `ABORT`; the repository
//! surface exposes no update operations for these tables (R3.6).

use std::path::Path;

use domain::ids::{
    AgentSpecId, CommandId, ConfigGenerationId, DaemonInstanceId, EnvironmentId, PrincipalId, RunId,
};
use domain::resource::WorkspaceAccessMode;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewResolvedBinding, NewResolvedEnvironment};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use sqlx::Sqlite;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";

fn config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 4,
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

fn new_environment(
    provider: &DeterministicIds,
    environment: EnvironmentId,
    run: RunId,
) -> NewResolvedEnvironment {
    NewResolvedEnvironment {
        environment_id: environment,
        run_id: run,
        agent_spec_id: AgentSpecId::new(provider),
        agent_spec_version: "1".to_owned(),
        agent_spec_digest: "spec-digest".to_owned(),
        agent_loop_id: "loop".to_owned(),
        agent_loop_version: "1".to_owned(),
        agent_loop_digest: "loop-digest".to_owned(),
        config_generation_id: ConfigGenerationId::new(provider),
        workspace_uri: Some("file:///workspace".to_owned()),
        workspace_base_revision: Some("abc123".to_owned()),
        workspace_mode: WorkspaceAccessMode::ReadOnly,
        model_provider: Some("provider".to_owned()),
        model_id: Some("model".to_owned()),
        model_parameters: Some(vec![1]),
        kernel_version: "0.1.0".to_owned(),
        protocol_versions: vec![2],
        capability_grant_ids: vec![3],
        approval_request_ids: vec![4],
        created_at_ms: 10,
    }
}

fn new_binding(provider: &DeterministicIds, port: &str) -> NewResolvedBinding {
    NewResolvedBinding {
        port_id: port.to_owned(),
        adapter_id: domain::ids::AdapterId::new(provider),
        adapter_version: "1".to_owned(),
        adapter_digest: "adapter-digest".to_owned(),
        capabilities: vec![9],
    }
}

#[tokio::test]
async fn environment_update_and_delete_are_rejected_by_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let environment = EnvironmentId::new(&provider);
    let run = RunId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.environments()
        .insert_environment(new_environment(&provider, environment, run))
        .await
        .unwrap();
    txn.environments()
        .insert_bindings(environment, vec![new_binding(&provider, "model")])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut conn = direct_connection(&db_path).await;
    let update = sqlx::query(
        "UPDATE resolved_run_environments SET kernel_version = 'tampered' \
         WHERE environment_id = ?",
    )
    .bind(environment.to_string())
    .execute(&mut *conn)
    .await
    .expect_err("environment updates must be rejected");
    let update_error = update.as_database_error().expect("database error");
    assert_eq!(update_error.code().as_deref(), Some("1811"));
    assert!(update_error.message().contains("immutable record"));

    let delete = sqlx::query("DELETE FROM resolved_run_environments WHERE environment_id = ?")
        .bind(environment.to_string())
        .execute(&mut *conn)
        .await
        .expect_err("environment deletes must be rejected");
    let delete_error = delete.as_database_error().expect("database error");
    assert_eq!(delete_error.code().as_deref(), Some("1811"));

    let binding_update =
        sqlx::query("UPDATE resolved_bindings SET adapter_version = '9' WHERE port_id = 'model'")
            .execute(&mut *conn)
            .await
            .expect_err("binding updates must be rejected");
    assert_eq!(
        binding_update
            .as_database_error()
            .expect("database error")
            .code()
            .as_deref(),
        Some("1811")
    );
    drop(conn);

    let mut read = store.begin_read().await.unwrap();
    let stored = read
        .environments()
        .get_environment(environment)
        .await
        .unwrap()
        .expect("environment is still present");
    assert_eq!(stored.kernel_version, "0.1.0");
    assert_eq!(stored.workspace_mode, WorkspaceAccessMode::ReadOnly);
    let bindings = read.environments().get_bindings(environment).await.unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].adapter_version, "1");
}

#[tokio::test]
async fn duplicate_environment_for_run_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let run = RunId::new(&provider);
    let first = EnvironmentId::new(&provider);
    let second = EnvironmentId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.environments()
        .insert_environment(new_environment(&provider, first, run))
        .await
        .unwrap();
    let duplicate = txn
        .environments()
        .insert_environment(new_environment(&provider, second, run))
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
    assert_eq!(duplicate.retry_class(), RetryClass::Never);
    drop(txn);

    let mut read = store.begin_read().await.unwrap();
    assert!(
        read.environments()
            .get_environment(first)
            .await
            .unwrap()
            .is_none(),
        "a failed transaction must leave zero rows"
    );
}

#[tokio::test]
async fn duplicate_bindings_are_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let run = RunId::new(&provider);
    let environment = EnvironmentId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.environments()
        .insert_environment(new_environment(&provider, environment, run))
        .await
        .unwrap();
    txn.environments()
        .insert_bindings(environment, vec![new_binding(&provider, "tool")])
        .await
        .unwrap();
    let duplicate = txn
        .environments()
        .insert_bindings(environment, vec![new_binding(&provider, "tool")])
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
    assert_eq!(duplicate.retry_class(), RetryClass::Never);
    txn.commit().await.unwrap();

    let mut read = store.begin_read().await.unwrap();
    let bindings = read.environments().get_bindings(environment).await.unwrap();
    assert_eq!(bindings.len(), 1, "the committed binding is untouched");
}

#[tokio::test]
async fn binding_without_environment_is_failed_precondition() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let missing = EnvironmentId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let orphan = txn
        .environments()
        .insert_bindings(missing, vec![new_binding(&provider, "tool")])
        .await
        .unwrap_err();
    assert_eq!(orphan.code(), ErrorCode::FailedPrecondition);
    assert_eq!(orphan.retry_class(), RetryClass::Never);
    drop(txn);

    let mut read = store.begin_read().await.unwrap();
    assert!(
        read.environments()
            .get_bindings(missing)
            .await
            .unwrap()
            .is_empty()
    );
}
