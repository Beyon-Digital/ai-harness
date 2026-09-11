//! Workspace repository: workspaces and epoch-guarded lease transitions.
//!
//! `cas_lease` checks the persisted lease epoch and applies the patch
//! atomically; a stale epoch reports `false` without mutation (R3.2). The
//! schema's partial unique index still rejects a second active exclusive
//! lease with `Conflict`.

use async_trait::async_trait;
use domain::ids::{LeaseId, WorkspaceId};
use domain::resource::{LeaseEnforcementState, WorkspaceAccessMode};
use kernel_store::models::{
    LeasePatch, NewWorkspace, NewWorkspaceLease, WorkspaceLeaseRow, WorkspaceRow,
};
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

const SELECT_LEASE: &str = "SELECT lease_id, workspace_id, owner_run_id, mode, lease_epoch, \
     enforcement_state, delegated_from, created_at_ms, updated_at_ms FROM workspace_leases";

fn decode_lease(row: &SqliteRow) -> errors::Result<WorkspaceLeaseRow> {
    Ok(WorkspaceLeaseRow {
        lease_id: mapping::decode_id(
            "workspace_leases.lease_id",
            &mapping::text(row, "lease_id")?,
        )?,
        workspace_id: mapping::decode_id(
            "workspace_leases.workspace_id",
            &mapping::text(row, "workspace_id")?,
        )?,
        owner_run_id: mapping::decode_id(
            "workspace_leases.owner_run_id",
            &mapping::text(row, "owner_run_id")?,
        )?,
        mode: mapping::decode_wire(
            "workspace_leases.mode",
            mapping::int(row, "mode")?,
            WorkspaceAccessMode::from_wire,
        )?,
        lease_epoch: mapping::decode_u64(
            "workspace_leases.lease_epoch",
            mapping::int(row, "lease_epoch")?,
        )?,
        enforcement_state: mapping::decode_state(
            "workspace_leases.enforcement_state",
            &mapping::text(row, "enforcement_state")?,
            LeaseEnforcementState::from_state_str,
        )?,
        delegated_from: mapping::blob(row, "delegated_from")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        updated_at_ms: mapping::int(row, "updated_at_ms")?,
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
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_LEASE} WHERE lease_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_lease).transpose()
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

    async fn insert_lease(&mut self, lease: NewWorkspaceLease) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO workspace_leases (lease_id, workspace_id, owner_run_id, mode, \
             lease_epoch, enforcement_state, delegated_from, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        )
        .bind(lease.lease_id.to_string())
        .bind(lease.workspace_id.to_string())
        .bind(lease.owner_run_id.to_string())
        .bind(i64::from(lease.mode.to_wire()))
        .bind(mapping::encode_u64(
            "workspace_leases.lease_epoch",
            lease.lease_epoch,
        )?)
        .bind(lease.enforcement_state.as_str())
        .bind(lease.delegated_from)
        .bind(lease.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_lease(
        &mut self,
        id: LeaseId,
        expect_epoch: u64,
        patch: LeasePatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let result = sqlx::query(
            "UPDATE workspace_leases SET \
             enforcement_state = COALESCE(?1, enforcement_state), \
             owner_run_id = COALESCE(?2, owner_run_id), \
             mode = COALESCE(?3, mode), \
             delegated_from = COALESCE(?4, delegated_from) \
             WHERE lease_id = ?5 AND lease_epoch = ?6",
        )
        .bind(patch.enforcement_state.map(|state| state.as_str()))
        .bind(patch.owner_run_id.map(|id| id.to_string()))
        .bind(patch.mode.map(|mode| i64::from(mode.to_wire())))
        .bind(patch.delegated_from)
        .bind(id.to_string())
        .bind(mapping::encode_u64(
            "workspace_leases.lease_epoch",
            expect_epoch,
        )?)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
