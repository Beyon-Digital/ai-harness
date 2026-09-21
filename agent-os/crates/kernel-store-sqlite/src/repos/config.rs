//! Configuration repository: generations and the active-pointer CAS.
//!
//! The active pointer is a singleton row. `cas_active` compares the persisted
//! revision (zero when no pointer exists) and rewrites the pointer at
//! `expected + 1`; a stale revision reports `false` without mutation (R3.2).

use async_trait::async_trait;
use domain::ids::ConfigGenerationId;
use domain::run::UnknownStateValue;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{ActiveConfigGenerationRow, ConfigGenerationRow, NewConfigGeneration};
use kernel_store::repositories::{ConfigRead, ConfigRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `config_generations` and `active_config_generation`.
pub(crate) struct SqliteConfigRepo {
    conn: SharedConn,
}

impl SqliteConfigRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

/// Parses the exact `config_generations.validation_state` CHECK literal set.
fn validation_state_from_state(value: &str) -> Result<String, UnknownStateValue> {
    match value {
        "proposed" | "validated" | "rejected" => Ok(value.to_owned()),
        _ => Err(UnknownStateValue {
            value: value.to_owned(),
            enum_name: "ConfigValidationState",
        }),
    }
}

/// Parses the exact `config_generations.test_state` CHECK literal set.
fn test_state_from_state(value: &str) -> Result<String, UnknownStateValue> {
    match value {
        "untested" | "passed" | "failed" => Ok(value.to_owned()),
        _ => Err(UnknownStateValue {
            value: value.to_owned(),
            enum_name: "ConfigTestState",
        }),
    }
}

fn decode_generation(row: &SqliteRow) -> errors::Result<ConfigGenerationRow> {
    Ok(ConfigGenerationRow {
        generation_id: mapping::decode_id(
            "config_generations.generation_id",
            &mapping::text(row, "generation_id")?,
        )?,
        digest: mapping::text(row, "digest")?,
        document: mapping::blob(row, "document")?,
        validation_state: mapping::decode_state(
            "config_generations.validation_state",
            &mapping::text(row, "validation_state")?,
            validation_state_from_state,
        )?,
        test_state: mapping::decode_state(
            "config_generations.test_state",
            &mapping::text(row, "test_state")?,
            test_state_from_state,
        )?,
        created_by_actor_id: mapping::decode_id(
            "config_generations.created_by_actor_id",
            &mapping::text(row, "created_by_actor_id")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

fn decode_active(row: &SqliteRow) -> errors::Result<ActiveConfigGenerationRow> {
    Ok(ActiveConfigGenerationRow {
        generation_id: mapping::decode_id(
            "active_config_generation.generation_id",
            &mapping::text(row, "generation_id")?,
        )?,
        revision: mapping::decode_u64(
            "active_config_generation.revision",
            mapping::int(row, "revision")?,
        )?,
        activated_at_ms: mapping::int(row, "activated_at_ms")?,
    })
}

#[async_trait]
impl ConfigRead for SqliteConfigRepo {
    async fn get_generation(
        &mut self,
        id: ConfigGenerationId,
    ) -> errors::Result<Option<ConfigGenerationRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT generation_id, digest, document, validation_state, test_state, \
             created_by_actor_id, created_at_ms FROM config_generations WHERE generation_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_generation).transpose()
    }

    async fn get_active(&mut self) -> errors::Result<Option<ActiveConfigGenerationRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT generation_id, revision, activated_at_ms FROM active_config_generation \
             WHERE singleton = 1",
        )
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_active).transpose()
    }
}

#[async_trait]
impl ConfigRepo for SqliteConfigRepo {
    async fn insert_generation(&mut self, generation: NewConfigGeneration) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO config_generations (generation_id, digest, document, validation_state, \
             test_state, created_by_actor_id, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(generation.generation_id.to_string())
        .bind(generation.digest)
        .bind(generation.document)
        .bind(generation.validation_state)
        .bind(generation.test_state)
        .bind(generation.created_by_actor_id.to_string())
        .bind(generation.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn set_generation_states(
        &mut self,
        id: ConfigGenerationId,
        validation_state: Option<&str>,
        test_state: Option<&str>,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let result = sqlx::query(
            "UPDATE config_generations SET                validation_state = COALESCE(?1, validation_state),                test_state = COALESCE(?2, test_state)              WHERE generation_id = ?3",
        )
        .bind(validation_state)
        .bind(test_state)
        .bind(id.to_string())
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "config generation not found",
            ));
        }
        Ok(())
    }

    async fn cas_active(
        &mut self,
        expected_revision: u64,
        generation: ConfigGenerationId,
        activated_at_ms: i64,
    ) -> errors::Result<bool> {
        // Read-then-upsert is race-free only because every writer holds
        // `BEGIN IMMEDIATE` on this connection; the revision cannot change
        // between the read below and the upsert.
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        let current: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM active_config_generation WHERE singleton = 1")
                .fetch_optional(&mut *conn)
                .await
                .map_err(mapping::from_sqlx)?;
        let current = match current {
            Some(value) => mapping::decode_u64("active_config_generation.revision", value)?,
            None => 0,
        };
        if current != expected_revision {
            return Ok(false);
        }

        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM config_generations WHERE generation_id = ?1")
                .bind(generation.to_string())
                .fetch_optional(&mut *conn)
                .await
                .map_err(mapping::from_sqlx)?;
        if exists.is_none() {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "config generation not found",
            ));
        }

        let next = mapping::encode_u64(
            "active_config_generation.revision",
            expected_revision.saturating_add(1),
        )?;
        let result = sqlx::query(
            "INSERT INTO active_config_generation (singleton, generation_id, revision, \
             activated_at_ms) VALUES (1, ?1, ?2, ?3) \
             ON CONFLICT(singleton) DO UPDATE SET \
               generation_id = excluded.generation_id, \
               revision = excluded.revision, \
               activated_at_ms = excluded.activated_at_ms",
        )
        .bind(generation.to_string())
        .bind(next)
        .bind(activated_at_ms)
        .execute(&mut *conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
