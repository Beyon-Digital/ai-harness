//! Agent spec repository: insert-only immutable revisions.
//!
//! The `agent_specs` table is append-only: the schema triggers reject every
//! update and delete, so this repository exposes no mutation beyond the first
//! insert of a revision. Re-inserting an identical `(id, version, digest,
//! body)` succeeds without writing; any other collision conflicts and leaves
//! the stored revision untouched.

use async_trait::async_trait;
use domain::ids::AgentSpecId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{AgentSpecRow, NewAgentSpec};
use kernel_store::repositories::{AgentSpecRead, AgentSpecRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `agent_specs` table.
pub(crate) struct SqliteAgentSpecRepo {
    conn: SharedConn,
}

impl SqliteAgentSpecRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_spec(row: &SqliteRow) -> errors::Result<AgentSpecRow> {
    Ok(AgentSpecRow {
        agent_spec_id: mapping::decode_id(
            "agent_specs.agent_spec_id",
            &mapping::text(row, "agent_spec_id")?,
        )?,
        version: mapping::text(row, "version")?,
        digest: mapping::text(row, "digest")?,
        body: mapping::blob(row, "body")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

async fn fetch_spec(
    conn: &mut sqlx::SqliteConnection,
    id: AgentSpecId,
    version: &str,
) -> errors::Result<Option<AgentSpecRow>> {
    let row = sqlx::query(
        "SELECT agent_spec_id, version, digest, body, created_at_ms FROM agent_specs \
         WHERE agent_spec_id = ?1 AND version = ?2",
    )
    .bind(id.to_string())
    .bind(version)
    .fetch_optional(conn)
    .await
    .map_err(mapping::from_sqlx)?;
    row.as_ref().map(decode_spec).transpose()
}

#[async_trait]
impl AgentSpecRead for SqliteAgentSpecRepo {
    async fn get(
        &mut self,
        id: AgentSpecId,
        version: &str,
    ) -> errors::Result<Option<AgentSpecRow>> {
        let mut guard = self.conn.lock().await;
        fetch_spec(guard.connection()?, id, version).await
    }
}

#[async_trait]
impl AgentSpecRepo for SqliteAgentSpecRepo {
    async fn insert(&mut self, spec: NewAgentSpec) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        if let Some(stored) = fetch_spec(conn, spec.agent_spec_id, &spec.version).await? {
            if stored.digest == spec.digest && stored.body == spec.body {
                return Ok(());
            }
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "agent spec revision conflicts with the stored revision",
            ));
        }
        sqlx::query(
            "INSERT INTO agent_specs (agent_spec_id, version, digest, body, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(spec.agent_spec_id.to_string())
        .bind(spec.version)
        .bind(spec.digest)
        .bind(spec.body)
        .bind(spec.created_at_ms)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
