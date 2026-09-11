//! Repository implementations over a shared SQLite connection.
//!
//! The groups implemented by the SQLite store are [`adapters`], [`artifacts`],
//! [`config`], [`effects`], [`environments`], [`graph`], [`loop_turns`],
//! [`resources`], [`runs`], [`security`], [`sessions`], [`tasks`], [`timers`],
//! and [`workspaces`]; each repository view shares the connection owned by its
//! transaction through [`SharedConn`].
//!
//! [`UnavailableRepo`] stands in for the idempotency and stream groups whose
//! SQL implementations arrive with PST-005. Every operation on it fails closed
//! with `FailedPrecondition`/`Never`, so a caller can never observe fabricated
//! rows.

pub(crate) mod adapters;
pub(crate) mod artifacts;
pub(crate) mod config;
pub(crate) mod effects;
pub(crate) mod environments;
pub(crate) mod graph;
pub(crate) mod loop_turns;
pub(crate) mod resources;
pub(crate) mod runs;
pub(crate) mod security;
pub(crate) mod sessions;
pub(crate) mod tasks;
pub(crate) mod timers;
pub(crate) mod workspaces;

use std::sync::Arc;

use domain::ids::*;
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

/// Repository view for groups without a SQL implementation in this store.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UnavailableRepo;

use kernel_store::models::*;
use kernel_store::repositories::*;

use crate::mapping;

macro_rules! unavailable_repo {
    ($($trait_name:ident { $(async fn $method:ident(&mut self $(, $arg:ident: $ty:ty)* $(,)?) -> errors::Result<$out:ty>;)* })*) => {
        $(
            #[async_trait::async_trait]
            impl $trait_name for UnavailableRepo {
                $(
                    async fn $method(&mut self $(, $arg: $ty)*) -> errors::Result<$out> {
                        $(let _ = $arg;)*
                        Err(mapping::unavailable(stringify!($trait_name)))
                    }
                )*
            }
        )*
    };
}

unavailable_repo! {
    IdempotencyRepo {
        async fn lookup(
            &mut self,
            principal: PrincipalId,
            key: &IdempotencyKey,
        ) -> errors::Result<Option<IdempotencyRecordRow>>;
        async fn insert(&mut self, record: NewIdempotencyRecord) -> errors::Result<()>;
    }
    StreamRepo {
        async fn allocate(&mut self, stream_key: EventStreamKey) -> errors::Result<u64>;
        async fn insert_outbox(&mut self, event: NewOutboxEvent) -> errors::Result<()>;
        async fn scan_unpublished(&mut self, limit: u32)
            -> errors::Result<Vec<OutboxEventRow>>;
        async fn mark_published(
            &mut self,
            event_id: EventId,
            kind: PublishKind,
        ) -> errors::Result<()>;
    }
}
