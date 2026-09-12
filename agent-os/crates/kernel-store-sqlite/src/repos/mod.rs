//! Repository implementations over a shared SQLite connection.
//!
//! The groups implemented by the SQLite store are [`adapters`], [`artifacts`],
//! [`config`], [`effects`], [`environments`], [`graph`], [`idempotency`],
//! [`loop_turns`], [`resources`], [`runs`], [`security`], [`sessions`],
//! [`streams`], [`tasks`], [`timers`], and [`workspaces`]; each repository
//! view shares the connection owned by its transaction through [`SharedConn`].

pub(crate) mod adapters;
pub(crate) mod artifacts;
pub(crate) mod config;
pub(crate) mod effects;
pub(crate) mod environments;
pub(crate) mod graph;
pub(crate) mod idempotency;
pub(crate) mod loop_turns;
pub(crate) mod resources;
pub(crate) mod runs;
pub(crate) mod security;
pub(crate) mod sessions;
pub(crate) mod streams;
pub(crate) mod tasks;
pub(crate) mod timers;
pub(crate) mod workspaces;

use std::sync::Arc;

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use sqlx::Sqlite;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::SqliteConnection;

/// Shared handle to the write connection owned by a write transaction.
pub(crate) type WriteConn = Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, Sqlite>>>>;

/// Shared handle to the read connection owned by a read transaction.
pub(crate) type ReadConn = Arc<tokio::sync::Mutex<Option<PoolConnection<Sqlite>>>>;

/// Connection shared by every repository view of one transaction.
#[derive(Clone)]
pub(crate) enum SharedConn {
    /// Connection inside an immediate write transaction.
    Write(WriteConn),
    /// Plain read connection.
    Read(ReadConn),
}

impl SharedConn {
    /// Locks the shared connection for the duration of one repository call.
    pub(crate) async fn lock(&self) -> ConnGuard<'_> {
        match self {
            Self::Write(conn) => ConnGuard::Write(conn.lock().await),
            Self::Read(conn) => ConnGuard::Read(conn.lock().await),
        }
    }
}

/// Held connection guard that yields the underlying connection.
pub(crate) enum ConnGuard<'a> {
    /// Guard over an immediate write transaction.
    Write(tokio::sync::MutexGuard<'a, Option<sqlx::Transaction<'static, Sqlite>>>),
    /// Guard over a plain read connection.
    Read(tokio::sync::MutexGuard<'a, Option<PoolConnection<Sqlite>>>),
}

impl ConnGuard<'_> {
    /// Borrows the live connection, or fails closed when the transaction has
    /// already been committed or rolled back.
    pub(crate) fn connection(&mut self) -> errors::Result<&mut SqliteConnection> {
        match self {
            Self::Write(guard) => guard.as_mut().map(|txn| &mut **txn),
            Self::Read(guard) => guard.as_mut().map(|conn| &mut **conn),
        }
        .ok_or_else(|| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "transaction connection is already finished",
            )
        })
    }
}
