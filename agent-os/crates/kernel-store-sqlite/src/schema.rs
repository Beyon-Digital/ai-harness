//! Verbatim inception-schema bootstrap and startup verification.

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection, SqlitePool};

/// The inception schema, executed verbatim exactly once per database file.
pub const SCHEMA: &str = include_str!("../../../schema/kernel_store.sql");

/// The single schema version this kernel accepts.
pub const SCHEMA_VERSION: &str = "1";

/// The exact table inventory created by the inception schema.
pub const TABLES: [&str; 30] = [
    "active_config_generation",
    "adapter_instances",
    "adapter_registrations",
    "agent_specs",
    "approval_requests",
    "approval_responses",
    "artifacts",
    "capability_grants",
    "conformance_reports",
    "config_generations",
    "daemon_fence",
    "decisions",
    "delegation_hops",
    "effects",
    "event_stream_heads",
    "idempotency_records",
    "kernel_meta",
    "loop_turns",
    "outbox_events",
    "resolved_bindings",
    "resolved_run_environments",
    "resource_reservations",
    "run_dependencies",
    "run_graph_heads",
    "runs",
    "sessions",
    "tasks",
    "timers",
    "workspace_leases",
    "workspaces",
];

/// Executes the inception schema on a freshly created database file.
pub async fn apply(options: &SqliteConnectOptions) -> errors::Result<()> {
    let mut connection = SqliteConnection::connect_with(options)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "kernel schema bootstrap connection failed",
            )
            .with_source(source)
        })?;
    sqlx::raw_sql(SCHEMA)
        .execute(&mut connection)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "kernel schema bootstrap failed",
            )
            .with_source(source)
        })?;
    connection.close().await.map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "kernel schema bootstrap connection failed to close",
        )
        .with_source(source)
    })
}

/// Verifies the version, table inventory, and PRAGMAs of an opened database.
pub async fn verify(pool: &SqlitePool, busy_timeout_ms: u64) -> errors::Result<()> {
    verify_version(pool).await?;
    verify_structure(pool).await?;
    verify_pragmas(pool, busy_timeout_ms).await
}

async fn verify_version(pool: &SqlitePool) -> errors::Result<()> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT value FROM kernel_meta WHERE key = 'schema_version'")
            .fetch_optional(pool)
            .await
            .map_err(|source| {
                KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    "kernel_meta schema_version is unreadable",
                )
                .with_source(source)
            })?;
    match found.as_deref() {
        Some(SCHEMA_VERSION) => Ok(()),
        Some(found) => Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("schema_version mismatch: found '{found}', expected '{SCHEMA_VERSION}'"),
        )),
        None => Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("schema_version missing: expected '{SCHEMA_VERSION}'"),
        )),
    }
}

async fn verify_structure(pool: &SqlitePool) -> errors::Result<()> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await
    .map_err(|source| {
        KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "kernel schema table inventory is unreadable",
        )
        .with_source(source)
    })?;
    for expected in TABLES {
        if !names.iter().any(|name| name == expected) {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                format!("schema table missing: {expected}"),
            ));
        }
    }
    if let Some(unexpected) = names.iter().find(|name| !TABLES.contains(&name.as_str())) {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("schema table unexpected: {unexpected}"),
        ));
    }
    Ok(())
}

async fn verify_pragmas(pool: &SqlitePool, busy_timeout_ms: u64) -> errors::Result<()> {
    let foreign_keys = pragma_i64(pool, "foreign_keys", "PRAGMA foreign_keys").await?;
    if foreign_keys != 1 {
        return Err(pragma_mismatch(
            "foreign_keys",
            "1",
            &foreign_keys.to_string(),
        ));
    }

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(pool)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "pragma journal_mode is unreadable",
            )
            .with_source(source)
        })?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(pragma_mismatch("journal_mode", "wal", &journal_mode));
    }

    let synchronous = pragma_i64(pool, "synchronous", "PRAGMA synchronous").await?;
    if synchronous != 2 {
        return Err(pragma_mismatch(
            "synchronous",
            "2",
            &synchronous.to_string(),
        ));
    }

    let busy_timeout = pragma_i64(pool, "busy_timeout", "PRAGMA busy_timeout").await?;
    let expected_timeout = busy_timeout_ms.min(i64::MAX as u64) as i64;
    if busy_timeout != expected_timeout {
        return Err(pragma_mismatch(
            "busy_timeout",
            &expected_timeout.to_string(),
            &busy_timeout.to_string(),
        ));
    }
    Ok(())
}

async fn pragma_i64(pool: &SqlitePool, name: &str, sql: &'static str) -> errors::Result<i64> {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                format!("pragma {name} is unreadable"),
            )
            .with_source(source)
        })
}

fn pragma_mismatch(name: &str, expected: &str, found: &str) -> KernelError {
    KernelError::new(
        ErrorCode::FailedPrecondition,
        RetryClass::Never,
        format!("pragma {name} not applied: expected {expected}, found {found}"),
    )
}
