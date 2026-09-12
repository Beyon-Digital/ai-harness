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
use kernel_store::types::{DaemonFence, TxContext};
use kernel_store::{KernelReadTxn, KernelStore, KernelTxn};
use sqlx::Sqlite;

use crate::SqliteKernelStore;
use crate::mapping;
use crate::repos::{
    ReadConn, SharedConn, WriteConn, adapters::SqliteAdapterRepo, artifacts::SqliteArtifactRepo,
    config::SqliteConfigRepo, effects::SqliteEffectRepo, environments::SqliteEnvironmentRepo,
    graph::SqliteGraphRepo, idempotency::SqliteIdempotencyRepo, loop_turns::SqliteLoopRepo,
    resources::SqliteResourceRepo, runs::SqliteRunRepo, security::SqliteSecurityRepo,
    sessions::SqliteSessionRepo, streams::SqliteStreamRepo, tasks::SqliteTaskRepo,
    timers::SqliteTimerRepo, workspaces::SqliteWorkspaceRepo,
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
    effects: SqliteEffectRepo,
    resources: SqliteResourceRepo,
    timers: SqliteTimerRepo,
    security: SqliteSecurityRepo,
    config: SqliteConfigRepo,
    workspaces: SqliteWorkspaceRepo,
    adapters: SqliteAdapterRepo,
    artifacts: SqliteArtifactRepo,
    loop_turns: SqliteLoopRepo,
    idempotency: SqliteIdempotencyRepo,
    streams: SqliteStreamRepo,
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
            environments: SqliteEnvironmentRepo::new(shared.clone()),
            effects: SqliteEffectRepo::new(shared.clone()),
            resources: SqliteResourceRepo::new(shared.clone()),
            timers: SqliteTimerRepo::new(shared.clone()),
            security: SqliteSecurityRepo::new(shared.clone()),
            config: SqliteConfigRepo::new(shared.clone()),
            workspaces: SqliteWorkspaceRepo::new(shared.clone()),
            adapters: SqliteAdapterRepo::new(shared.clone()),
            artifacts: SqliteArtifactRepo::new(shared.clone()),
            loop_turns: SqliteLoopRepo::new(shared.clone()),
            idempotency: SqliteIdempotencyRepo::new(shared.clone()),
            streams: SqliteStreamRepo::new(shared),
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
        &mut self.effects
    }

    fn resources(&mut self) -> &mut dyn ResourceRepo {
        &mut self.resources
    }

    fn timers(&mut self) -> &mut dyn TimerRepo {
        &mut self.timers
    }

    fn security(&mut self) -> &mut dyn SecurityRepo {
        &mut self.security
    }

    fn config(&mut self) -> &mut dyn ConfigRepo {
        &mut self.config
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRepo {
        &mut self.workspaces
    }

    fn adapters(&mut self) -> &mut dyn AdapterRepo {
        &mut self.adapters
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRepo {
        &mut self.artifacts
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRepo {
        &mut self.loop_turns
    }

    fn idempotency(&mut self) -> &mut dyn IdempotencyRepo {
        &mut self.idempotency
    }

    fn streams(&mut self) -> &mut dyn StreamRepo {
        &mut self.streams
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
    effects: SqliteEffectRepo,
    resources: SqliteResourceRepo,
    timers: SqliteTimerRepo,
    security: SqliteSecurityRepo,
    config: SqliteConfigRepo,
    workspaces: SqliteWorkspaceRepo,
    adapters: SqliteAdapterRepo,
    artifacts: SqliteArtifactRepo,
    loop_turns: SqliteLoopRepo,
}

impl SqliteReadTxn {
    fn new(conn: ReadConn) -> Self {
        let shared = SharedConn::Read(conn);
        Self {
            runs: SqliteRunRepo::new(shared.clone()),
            tasks: SqliteTaskRepo::new(shared.clone()),
            sessions: SqliteSessionRepo::new(shared.clone()),
            graph: SqliteGraphRepo::new(shared.clone()),
            environments: SqliteEnvironmentRepo::new(shared.clone()),
            effects: SqliteEffectRepo::new(shared.clone()),
            resources: SqliteResourceRepo::new(shared.clone()),
            timers: SqliteTimerRepo::new(shared.clone()),
            security: SqliteSecurityRepo::new(shared.clone()),
            config: SqliteConfigRepo::new(shared.clone()),
            workspaces: SqliteWorkspaceRepo::new(shared.clone()),
            adapters: SqliteAdapterRepo::new(shared.clone()),
            artifacts: SqliteArtifactRepo::new(shared.clone()),
            loop_turns: SqliteLoopRepo::new(shared),
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
        &mut self.effects
    }

    fn resources(&mut self) -> &mut dyn ResourceRead {
        &mut self.resources
    }

    fn timers(&mut self) -> &mut dyn TimerRead {
        &mut self.timers
    }

    fn security(&mut self) -> &mut dyn SecurityRead {
        &mut self.security
    }

    fn config(&mut self) -> &mut dyn ConfigRead {
        &mut self.config
    }

    fn workspaces(&mut self) -> &mut dyn WorkspaceRead {
        &mut self.workspaces
    }

    fn adapters(&mut self) -> &mut dyn AdapterRead {
        &mut self.adapters
    }

    fn artifacts(&mut self) -> &mut dyn ArtifactRead {
        &mut self.artifacts
    }

    fn loop_turns(&mut self) -> &mut dyn LoopRead {
        &mut self.loop_turns
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
        crate::fence::assert_epoch(&mut txn, ctx.daemon_epoch).await?;
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
        crate::fence::claim(self.pool(), instance, crate::fence::FENCE_LEASE_MS).await
    }

    async fn current_fence(&self) -> errors::Result<Option<DaemonFence>> {
        crate::fence::current(self.pool()).await
    }
}
