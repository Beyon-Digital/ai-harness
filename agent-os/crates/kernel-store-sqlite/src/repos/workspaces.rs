//! Workspace repository: workspace rows; epoch-guarded lease transitions
//! live in `workspace_leases.rs` (WRK-003).

use async_trait::async_trait;
use domain::ids::{LeaseId, WorkspaceId};
use kernel_store::models::{NewWorkspace, WorkspaceLeaseRow, WorkspaceRow};
use kernel_store::repositories::{WorkspaceRead, WorkspaceRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `workspaces` and `workspace_leases`.
pub(crate) struct SqliteWorkspaceRepo {
    conn: SharedConn,
}

impl SqliteWorkspaceRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }

    /// Shared connection for the lease module's impl blocks.
    pub(crate) fn conn(&self) -> &SharedConn {
        &self.conn
    }
}

fn decode_workspace(row: &SqliteRow) -> errors::Result<WorkspaceRow> {
    Ok(WorkspaceRow {
        workspace_id: mapping::decode_id(
            "workspaces.workspace_id",
            &mapping::text(row, "workspace_id")?,
        )?,
        kind: mapping::text(row, "kind")?,
        base_revision: mapping::opt_text(row, "base_revision")?,
        parent_workspace_id: mapping::decode_opt_id(
            "workspaces.parent_workspace_id",
            mapping::opt_text(row, "parent_workspace_id")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

#[async_trait]
impl WorkspaceRead for SqliteWorkspaceRepo {
    async fn get_workspace(&mut self, id: WorkspaceId) -> errors::Result<Option<WorkspaceRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT workspace_id, kind, base_revision, parent_workspace_id, created_at_ms \
             FROM workspaces WHERE workspace_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_workspace).transpose()
    }

    async fn get_lease(&mut self, id: LeaseId) -> errors::Result<Option<WorkspaceLeaseRow>> {
        self.get_lease_impl(id).await
    }
}

#[async_trait]
impl WorkspaceRepo for SqliteWorkspaceRepo {
    async fn insert_workspace(&mut self, workspace: NewWorkspace) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, kind, base_revision, parent_workspace_id, \
             created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(workspace.workspace_id.to_string())
        .bind(workspace.kind)
        .bind(workspace.base_revision)
        .bind(workspace.parent_workspace_id.map(|id| id.to_string()))
        .bind(workspace.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_lease(
        &mut self,
        lease: kernel_store::models::NewWorkspaceLease,
    ) -> errors::Result<()> {
        self.insert_lease_impl(lease).await
    }

    async fn cas_lease(
        &mut self,
        id: LeaseId,
        expect_epoch: u64,
        patch: kernel_store::models::LeasePatch,
    ) -> errors::Result<bool> {
        self.cas_lease_impl(id, expect_epoch, patch).await
    }
}
