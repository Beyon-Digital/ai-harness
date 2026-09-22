//! Workspace lease persistence: epoch-guarded transitions live here so
//! exclusive-transfer CAS and stale-token rejection stay auditable in
//! one place (WRK-003).
//!
//! `cas_lease` checks the persisted lease epoch and applies the patch
//! atomically; a stale epoch reports `false` without mutation (R3.2). The
//! schema's partial unique index still rejects a second active exclusive
//! lease with `Conflict`. A patch that sets `lease_epoch` advances the
//! epoch in the same statement, so any handle holding the old epoch is
//! invalidated the moment the transfer commits.

use domain::ids::LeaseId;
use domain::resource::{LeaseEnforcementState, WorkspaceAccessMode};
use kernel_store::models::{LeasePatch, NewWorkspaceLease, WorkspaceLeaseRow};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::workspaces::SqliteWorkspaceRepo;

const SELECT_LEASE: &str = "SELECT lease_id, workspace_id, owner_run_id, mode, lease_epoch, \
     enforcement_state, delegated_from, created_at_ms, updated_at_ms FROM workspace_leases";

pub(crate) fn decode_lease(row: &SqliteRow) -> errors::Result<WorkspaceLeaseRow> {
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

impl SqliteWorkspaceRepo {
    pub(crate) async fn get_lease_impl(
        &mut self,
        id: LeaseId,
    ) -> errors::Result<Option<WorkspaceLeaseRow>> {
        let mut guard = self.conn().lock().await;
        let query = format!("{SELECT_LEASE} WHERE lease_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_lease).transpose()
    }
}

/// Lease-half of the workspace repo; wired into `WorkspaceRepo` here so
/// lease writes live in this module.
impl SqliteWorkspaceRepo {
    pub(crate) async fn insert_lease_impl(
        &mut self,
        lease: NewWorkspaceLease,
    ) -> errors::Result<()> {
        let mut guard = self.conn().lock().await;
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

    pub(crate) async fn cas_lease_impl(
        &mut self,
        id: LeaseId,
        expect_epoch: u64,
        patch: LeasePatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn().lock().await;
        let result = sqlx::query(
            "UPDATE workspace_leases SET \
             enforcement_state = COALESCE(?1, enforcement_state), \
             owner_run_id = COALESCE(?2, owner_run_id), \
             mode = COALESCE(?3, mode), \
             delegated_from = COALESCE(?4, delegated_from), \
             lease_epoch = COALESCE(?5, lease_epoch) \
             WHERE lease_id = ?6 AND lease_epoch = ?7",
        )
        .bind(patch.enforcement_state.map(|state| state.as_str()))
        .bind(patch.owner_run_id.map(|id| id.to_string()))
        .bind(patch.mode.map(|mode| i64::from(mode.to_wire())))
        .bind(patch.delegated_from)
        .bind(match patch.lease_epoch {
            Some(epoch) => Some(mapping::encode_u64("workspace_leases.lease_epoch", epoch)?),
            None => None,
        })
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
