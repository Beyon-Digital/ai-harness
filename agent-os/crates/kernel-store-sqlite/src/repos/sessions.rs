//! Session repository: insert and fetch.

use async_trait::async_trait;
use domain::ids::SessionId;
use kernel_store::models::{NewSession, SessionRow};
use kernel_store::repositories::{SessionRead, SessionRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `sessions` table.
pub(crate) struct SqliteSessionRepo {
    conn: SharedConn,
}

impl SqliteSessionRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_session(row: &SqliteRow) -> errors::Result<SessionRow> {
    Ok(SessionRow {
        session_id: mapping::decode_id("sessions.session_id", &mapping::text(row, "session_id")?)?,
        principal_id: mapping::decode_id(
            "sessions.principal_id",
            &mapping::text(row, "principal_id")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        metadata: mapping::opt_blob(row, "metadata")?,
    })
}

#[async_trait]
impl SessionRead for SqliteSessionRepo {
    async fn get(&mut self, id: SessionId) -> errors::Result<Option<SessionRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT session_id, principal_id, created_at_ms, metadata \
             FROM sessions WHERE session_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_session).transpose()
    }
}

#[async_trait]
impl SessionRepo for SqliteSessionRepo {
    async fn insert(&mut self, session: NewSession) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO sessions (session_id, principal_id, created_at_ms, metadata) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(session.session_id.to_string())
        .bind(session.principal_id.to_string())
        .bind(session.created_at_ms)
        .bind(session.metadata)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
