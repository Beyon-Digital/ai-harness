//! SQLite schema, repositories, and transactions implementing the kernel store port.
#![forbid(unsafe_code)]

pub mod fence;
pub mod mapping;
pub mod repos;
pub mod schema;
pub mod txn;

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Configuration for [`SqliteKernelStore::open`].
#[derive(Clone, Debug)]
pub struct StoreConfig {
    /// Path of the `kernel.db` file; its parent directory is the runtime directory.
    pub path: PathBuf,
    /// Maximum number of pooled connections; values below one are treated as one.
    pub pool_max_connections: u32,
    /// SQLite busy timeout applied to every pooled connection, in milliseconds.
    pub busy_timeout_ms: u64,
}

/// SQLite-backed kernel store.
#[derive(Debug)]
pub struct SqliteKernelStore {
    pool: SqlitePool,
    path: PathBuf,
}

impl SqliteKernelStore {
    /// Creates `kernel.db` from the inception schema on first open, otherwise verifies it.
    ///
    /// The runtime directory (the parent of `config.path`) is created with mode `0700` and
    /// the database file with mode `0600`; both modes are re-asserted on every open.
    pub async fn open(config: StoreConfig) -> errors::Result<Self> {
        let StoreConfig {
            path,
            pool_max_connections,
            busy_timeout_ms,
        } = config;
        let runtime_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    "kernel database path has no parent directory",
                )
            })?;
        prepare_runtime_dir(runtime_dir)?;
        let created = prepare_database_file(&path)?;
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true);
        if created {
            schema::apply(&options).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(pool_max_connections.max(1))
            .connect_with(options)
            .await
            .map_err(|source| {
                KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Safe,
                    "kernel database connection failed",
                )
                .with_source(source)
            })?;
        let store = Self { pool, path };
        schema::verify(&store.pool, busy_timeout_ms).await?;
        Ok(store)
    }

    /// Returns the path of the `kernel.db` file this store opened.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrows the pooled connections backing this store.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn prepare_runtime_dir(dir: &Path) -> errors::Result<()> {
    fs::create_dir_all(dir).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "kernel runtime directory could not be created",
        )
        .with_source(source)
    })?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "kernel runtime directory permissions could not be set",
        )
        .with_source(source)
    })
}

fn prepare_database_file(path: &Path) -> errors::Result<bool> {
    let created = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => true,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(source) => {
            return Err(KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "kernel database file could not be created",
            )
            .with_source(source));
        }
    };
    if !created && !path.is_file() {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "kernel database path is not a regular file",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "kernel database file permissions could not be set",
        )
        .with_source(source)
    })?;
    Ok(created)
}
