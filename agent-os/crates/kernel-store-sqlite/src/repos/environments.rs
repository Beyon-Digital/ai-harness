//! Immutable environment repository: resolved environments and bindings.
//!
//! Both tables reject updates and deletes through schema triggers, so the
//! repository exposes insertion and reads only (R3.6).

use async_trait::async_trait;
use domain::ids::EnvironmentId;
use domain::resource::WorkspaceAccessMode;
use kernel_store::models::{
    NewResolvedBinding, NewResolvedEnvironment, ResolvedBindingRow, ResolvedEnvironmentRow,
};
use kernel_store::repositories::{EnvironmentRead, EnvironmentRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `resolved_run_environments` and `resolved_bindings`.
pub(crate) struct SqliteEnvironmentRepo {
    conn: SharedConn,
}

impl SqliteEnvironmentRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_environment(row: &SqliteRow) -> errors::Result<ResolvedEnvironmentRow> {
    Ok(ResolvedEnvironmentRow {
        environment_id: mapping::decode_id(
            "resolved_run_environments.environment_id",
            &mapping::text(row, "environment_id")?,
        )?,
        run_id: mapping::decode_id(
            "resolved_run_environments.run_id",
            &mapping::text(row, "run_id")?,
        )?,
        agent_spec_id: mapping::decode_id(
            "resolved_run_environments.agent_spec_id",
            &mapping::text(row, "agent_spec_id")?,
        )?,
        agent_spec_version: mapping::text(row, "agent_spec_version")?,
        agent_spec_digest: mapping::text(row, "agent_spec_digest")?,
        agent_loop_id: mapping::text(row, "agent_loop_id")?,
        agent_loop_version: mapping::text(row, "agent_loop_version")?,
        agent_loop_digest: mapping::text(row, "agent_loop_digest")?,
        config_generation_id: mapping::decode_id(
            "resolved_run_environments.config_generation_id",
            &mapping::text(row, "config_generation_id")?,
        )?,
        workspace_uri: mapping::opt_text(row, "workspace_uri")?,
        workspace_base_revision: mapping::opt_text(row, "workspace_base_revision")?,
        workspace_mode: mapping::decode_wire(
            "resolved_run_environments.workspace_mode",
            mapping::int(row, "workspace_mode")?,
            WorkspaceAccessMode::from_wire,
        )?,
        model_provider: mapping::opt_text(row, "model_provider")?,
        model_id: mapping::opt_text(row, "model_id")?,
        model_parameters: mapping::opt_blob(row, "model_parameters")?,
        kernel_version: mapping::text(row, "kernel_version")?,
        protocol_versions: mapping::blob(row, "protocol_versions")?,
        capability_grant_ids: mapping::blob(row, "capability_grant_ids")?,
        approval_request_ids: mapping::blob(row, "approval_request_ids")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

fn decode_binding(row: &SqliteRow) -> errors::Result<ResolvedBindingRow> {
    Ok(ResolvedBindingRow {
        environment_id: mapping::decode_id(
            "resolved_bindings.environment_id",
            &mapping::text(row, "environment_id")?,
        )?,
        port_id: mapping::text(row, "port_id")?,
        adapter_id: mapping::decode_id(
            "resolved_bindings.adapter_id",
            &mapping::text(row, "adapter_id")?,
        )?,
        adapter_version: mapping::text(row, "adapter_version")?,
        adapter_digest: mapping::text(row, "adapter_digest")?,
        capabilities: mapping::blob(row, "capabilities")?,
    })
}

#[async_trait]
impl EnvironmentRead for SqliteEnvironmentRepo {
    async fn get_environment(
        &mut self,
        id: EnvironmentId,
    ) -> errors::Result<Option<ResolvedEnvironmentRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT environment_id, run_id, agent_spec_id, agent_spec_version, agent_spec_digest, \
             agent_loop_id, agent_loop_version, agent_loop_digest, config_generation_id, \
             workspace_uri, workspace_base_revision, workspace_mode, model_provider, model_id, \
             model_parameters, kernel_version, protocol_versions, capability_grant_ids, \
             approval_request_ids, created_at_ms \
             FROM resolved_run_environments WHERE environment_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_environment).transpose()
    }

    async fn get_bindings(&mut self, id: EnvironmentId) -> errors::Result<Vec<ResolvedBindingRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT environment_id, port_id, adapter_id, adapter_version, adapter_digest, \
             capabilities FROM resolved_bindings WHERE environment_id = ?1 ORDER BY port_id",
        )
        .bind(id.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_binding).collect()
    }
}

#[async_trait]
impl EnvironmentRepo for SqliteEnvironmentRepo {
    async fn insert_environment(&mut self, new: NewResolvedEnvironment) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO resolved_run_environments (environment_id, run_id, agent_spec_id, \
             agent_spec_version, agent_spec_digest, agent_loop_id, agent_loop_version, \
             agent_loop_digest, config_generation_id, workspace_uri, workspace_base_revision, \
             workspace_mode, model_provider, model_id, model_parameters, kernel_version, \
             protocol_versions, capability_grant_ids, approval_request_ids, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
             ?17, ?18, ?19, ?20)",
        )
        .bind(new.environment_id.to_string())
        .bind(new.run_id.to_string())
        .bind(new.agent_spec_id.to_string())
        .bind(new.agent_spec_version)
        .bind(new.agent_spec_digest)
        .bind(new.agent_loop_id)
        .bind(new.agent_loop_version)
        .bind(new.agent_loop_digest)
        .bind(new.config_generation_id.to_string())
        .bind(new.workspace_uri)
        .bind(new.workspace_base_revision)
        .bind(i64::from(new.workspace_mode.to_wire()))
        .bind(new.model_provider)
        .bind(new.model_id)
        .bind(new.model_parameters)
        .bind(new.kernel_version)
        .bind(new.protocol_versions)
        .bind(new.capability_grant_ids)
        .bind(new.approval_request_ids)
        .bind(new.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_bindings(
        &mut self,
        environment: EnvironmentId,
        bindings: Vec<NewResolvedBinding>,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        for binding in bindings {
            sqlx::query(
                "INSERT INTO resolved_bindings (environment_id, port_id, adapter_id, \
                 adapter_version, adapter_digest, capabilities) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(environment.to_string())
            .bind(binding.port_id)
            .bind(binding.adapter_id.to_string())
            .bind(binding.adapter_version)
            .bind(binding.adapter_digest)
            .bind(binding.capabilities)
            .execute(&mut *conn)
            .await
            .map_err(mapping::from_sqlx)?;
        }
        Ok(())
    }
}
