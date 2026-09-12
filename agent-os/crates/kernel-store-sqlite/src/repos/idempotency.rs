//! Idempotency repository: scoped replay lookup and insert over
//! `idempotency_records` (R5.1-R5.4).
//!
//! The unique `(principal_id, idempotency_key)` primary key is the single
//! authority for replay safety: a second insert for the same key is rejected
//! by the storage constraint and maps to `Conflict` (D5), with no pre-check
//! race between a lookup and the insert.

use async_trait::async_trait;
use domain::ids::{IdempotencyKey, PrincipalId};
use kernel_store::models::{IdempotencyRecordRow, NewIdempotencyRecord};
use kernel_store::repositories::IdempotencyRepo;
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `idempotency_records` table.
pub(crate) struct SqliteIdempotencyRepo {
    conn: SharedConn,
}

impl SqliteIdempotencyRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_record(row: &SqliteRow) -> errors::Result<IdempotencyRecordRow> {
    Ok(IdempotencyRecordRow {
        principal_id: mapping::decode_id(
            "idempotency_records.principal_id",
            &mapping::text(row, "principal_id")?,
        )?,
        idempotency_key: mapping::decode_id(
            "idempotency_records.idempotency_key",
            &mapping::text(row, "idempotency_key")?,
        )?,
        request_digest: mapping::text(row, "request_digest")?,
        command_id: mapping::decode_id(
            "idempotency_records.command_id",
            &mapping::text(row, "command_id")?,
        )?,
        outcome_code: mapping::text(row, "outcome_code")?,
        outcome_payload: mapping::blob(row, "outcome_payload")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

#[async_trait]
impl IdempotencyRepo for SqliteIdempotencyRepo {
    async fn lookup(
        &mut self,
        principal: PrincipalId,
        key: &IdempotencyKey,
    ) -> errors::Result<Option<IdempotencyRecordRow>> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        let row = sqlx::query(
            "SELECT principal_id, idempotency_key, request_digest, command_id, \
             outcome_code, outcome_payload, created_at_ms FROM idempotency_records \
             WHERE principal_id = ?1 AND idempotency_key = ?2",
        )
        .bind(principal.to_string())
        .bind(key.as_str())
        .fetch_optional(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_record).transpose()
    }

    async fn insert(&mut self, record: NewIdempotencyRecord) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        sqlx::query(
            "INSERT INTO idempotency_records (principal_id, idempotency_key, \
             request_digest, command_id, outcome_code, outcome_payload, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(record.principal_id.to_string())
        .bind(record.idempotency_key.as_str())
        .bind(record.request_digest)
        .bind(record.command_id.to_string())
        .bind(record.outcome_code)
        .bind(record.outcome_payload)
        .bind(record.created_at_ms)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
