//! Write and read transaction guards over pooled SQLite connections.
//!
//! `begin_write` reserves writer ordering with an explicit `BEGIN IMMEDIATE`
//! on its own pooled connection (D2, R3.3) and asserts the persisted daemon
//! epoch exactly once, before any statement runs in the transaction (D6,
//! R4.3). `commit` executes `COMMIT`; `rollback` executes `ROLLBACK`; dropping
//! an unfinished guard rolls back through the sqlx transaction guard, which
//! queues a rollback on the owned connection.
//!
//! `begin_read` acquires a plain pooled connection and issues no write
//! statements.

use std::sync::Arc;

use async_trait::async_trait;
use domain::ids::DaemonInstanceId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::repositories::*;
use kernel_store::types::{DaemonEpoch, DaemonFence, TxContext};
use kernel_store::{KernelReadTxn, KernelStore, KernelTxn};
use sqlx::Sqlite;

use crate::SqliteKernelStore;
use crate::mapping;
use crate::repos::{
    ReadConn, SharedConn, UnavailableRepo, WriteConn, environments::SqliteEnvironmentRepo,
    graph::SqliteGraphRepo, runs::SqliteRunRepo, sessions::SqliteSessionRepo,
    tasks::SqliteTaskRepo,
};

/// Write transaction guard owning its `BEGIN IMMEDIATE` connection.
pub(crate) struct SqliteWriteTxn {
    ctx: TxContext,
    conn: WriteConn,
    runs: SqliteRunRepo,
    tasks: SqliteTaskRepo,
    sessions: SqliteSessionRepo,
    graph: SqliteGraphRepo,
    environments: SqliteEnvironmentRepo,
    unavailable: UnavailableRepo,
}

impl SqliteWriteTxn {
    fn new(ctx: TxContext, txn: sqlx::Transaction<'static, Sqlite>) -> Self {
        let conn: WriteConn = Arc::new(tokio::sync::Mutex::new(Some(txn)));
        let shared = SharedConn::Write(conn.clone());
        Self {
            ctx,
            conn,
            runs: SqliteRunRepo::new(shared.clone()),
            tasks: SqliteTaskRepo::new(shared.clone()),
            sessions: SqliteSessionRepo::new(shared.clone()),
            graph: SqliteGraphRepo::new(shared.clone()),
            environments: SqliteEnvironmentRepo::new(shared),
            unavailable: UnavailableRepo,
        }
    }

    async fn take(&self) -> errors::Result<sqlx::Transaction<'static, Sqlite>> {
        let mut guard = self.conn.lock().await;
        guard.take().ok_or_else(|| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "write transaction is already finished",
            )
        })
    }
}

#[async_trait]
impl KernelTxn for SqliteWriteTxn {
    fn context(&self) -> &TxContext {
        &self.ctx
    }

    fn runs(&mut self) -> &mut dyn RunRepo {
        &mut self.runs
    }

    fn tasks(&mut self) -> &mut dyn TaskRepo {
        &mut self.tasks
    }

    fn sessions(&mut self) -> &mut dyn SessionRepo {
        &mut self.sessions
    }

    fn graph(&mut self) -> &mut dyn GraphRepo {
        &mut self.graph
    }

    fn environments(&mut self) -> &mut dyn EnvironmentRepo {
        &mut self.environments
    }

    fn effects(&mut self) -> &mut dyn EffectRepo {
        &mut self.unavailable
    }

    fn resources(&mut self) -> &mut dyn ResourceRepo {
        &mut self.unavailable
    }

    fn timers(&mut self) -> &mut dyn TimerRepo {
        &mut self.unavailable
    }

    fn security(&mut self) -> &mut dyn SecurityRepo {
        &mut self.unavailable
    }

    fn config(&mut self) -> &mut dyn ConfigRepo {
        &mut self.unavailable
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRepo {
        &mut self.unavailable
    }

    fn adapters(&mut self) -> &mut dyn AdapterRepo {
        &mut self.unavailable
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRepo {
        &mut self.unavailable
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRepo {
        &mut self.unavailable
    }

    fn idempotency(&mut self) -> &mut dyn IdempotencyRepo {
        &mut self.unavailable
    }

    fn streams(&mut self) -> &mut dyn StreamRepo {
        &mut self.unavailable
    }

    async fn commit(self: Box<Self>) -> errors::Result<()> {
        let txn = self.take().await?;
        txn.commit().await.map_err(mapping::from_sqlx)
    }

    async fn rollback(self: Box<Self>) -> errors::Result<()> {
        let txn = self.take().await?;
        txn.rollback().await.map_err(mapping::from_sqlx)
    }
}

/// Read transaction guard owning a plain pooled connection.
pub(crate) struct SqliteReadTxn {
    runs: SqliteRunRepo,
    tasks: SqliteTaskRepo,
    sessions: SqliteSessionRepo,
    graph: SqliteGraphRepo,
    environments: SqliteEnvironmentRepo,
    unavailable: UnavailableRepo,
}

impl SqliteReadTxn {
    fn new(conn: ReadConn) -> Self {
        let shared = SharedConn::Read(conn);
        Self {
            runs: SqliteRunRepo::new(shared.clone()),
            tasks: SqliteTaskRepo::new(shared.clone()),
            sessions: SqliteSessionRepo::new(shared.clone()),
            graph: SqliteGraphRepo::new(shared.clone()),
            environments: SqliteEnvironmentRepo::new(shared),
            unavailable: UnavailableRepo,
        }
    }
}

#[async_trait]
impl KernelReadTxn for SqliteReadTxn {
    fn runs(&mut self) -> &mut dyn RunRead {
        &mut self.runs
    }

    fn tasks(&mut self) -> &mut dyn TaskRead {
        &mut self.tasks
    }

    fn sessions(&mut self) -> &mut dyn SessionRead {
        &mut self.sessions
    }

    fn graph(&mut self) -> &mut dyn GraphRead {
        &mut self.graph
    }

    fn environments(&mut self) -> &mut dyn EnvironmentRead {
        &mut self.environments
    }

    fn effects(&mut self) -> &mut dyn EffectRead {
        &mut self.unavailable
    }

    fn resources(&mut self) -> &mut dyn ResourceRead {
        &mut self.unavailable
    }

    fn timers(&mut self) -> &mut dyn TimerRead {
        &mut self.unavailable
    }

    fn security(&mut self) -> &mut dyn SecurityRead {
        &mut self.unavailable
    }

    fn config(&mut self) -> &mut dyn ConfigRead {
        &mut self.unavailable
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRead {
        &mut self.unavailable
    }

    fn adapters(&mut self) -> &mut dyn AdapterRead {
        &mut self.unavailable
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRead {
        &mut self.unavailable
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRead {
        &mut self.unavailable
    }
}

#[async_trait]
impl KernelStore for SqliteKernelStore {
    async fn begin_write(&self, ctx: TxContext) -> errors::Result<Box<dyn KernelTxn + '_>> {
        let mut txn = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(mapping::from_sqlx)?;
        let persisted: Option<i64> =
            sqlx::query_scalar("SELECT fencing_epoch FROM daemon_fence WHERE singleton = 1")
                .fetch_optional(&mut *txn)
                .await
                .map_err(mapping::from_sqlx)?;
        let expected = i64::try_from(ctx.daemon_epoch).ok();
        if persisted.is_none() || persisted != expected {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "write transaction rejected: daemon epoch is absent or stale",
            ));
        }
        Ok(Box::new(SqliteWriteTxn::new(ctx, txn)))
    }

    async fn begin_read(&self) -> errors::Result<Box<dyn KernelReadTxn + '_>> {
        let conn = self.pool().acquire().await.map_err(mapping::from_sqlx)?;
        let handle: ReadConn = Arc::new(tokio::sync::Mutex::new(Some(conn)));
        Ok(Box::new(SqliteReadTxn::new(handle)))
    }

    async fn acquire_daemon_fence(
        &self,
        instance: DaemonInstanceId,
    ) -> errors::Result<DaemonFence> {
        let mut txn = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(mapping::from_sqlx)?;
        let epoch: i64 = sqlx::query_scalar(
            "INSERT INTO daemon_fence (singleton, instance_id, fencing_epoch, lease_expires_ms, \
             updated_at_ms) VALUES (1, ?1, 1, ?2, ?3) \
             ON CONFLICT(singleton) DO UPDATE SET \
               instance_id = excluded.instance_id, \
               fencing_epoch = fencing_epoch + 1, \
               lease_expires_ms = excluded.lease_expires_ms, \
               updated_at_ms = excluded.updated_at_ms \
             RETURNING fencing_epoch",
        )
        .bind(instance.to_string())
        .bind(i64::MAX)
        .bind(mapping::unix_ms_now()?)
        .fetch_one(&mut *txn)
        .await
        .map_err(mapping::from_sqlx)?;
        txn.commit().await.map_err(mapping::from_sqlx)?;
        Ok(DaemonFence {
            instance_id: instance,
            epoch: DaemonEpoch(mapping::decode_u64("daemon_fence.fencing_epoch", epoch)?),
            lease_expires_unix_ms: i64::MAX,
        })
    }

    async fn current_fence(&self) -> errors::Result<Option<DaemonFence>> {
        let row: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT instance_id, fencing_epoch, lease_expires_ms FROM daemon_fence \
             WHERE singleton = 1",
        )
        .fetch_optional(self.pool())
        .await
        .map_err(mapping::from_sqlx)?;
        row.map(|(instance, epoch, lease_expires_unix_ms)| {
            Ok(DaemonFence {
                instance_id: mapping::decode_id("daemon_fence.instance_id", &instance)?,
                epoch: DaemonEpoch(mapping::decode_u64("daemon_fence.fencing_epoch", epoch)?),
                lease_expires_unix_ms,
            })
        })
        .transpose()
    }
}
