//! Artifact repository: immutable inserts with id- and URI-based reads.

use async_trait::async_trait;
use domain::ids::{ArtifactId, RunId};
use domain::security::{RetentionClass, SensitivityClass};
use kernel_store::models::{ArtifactRow, NewArtifact};
use kernel_store::repositories::{ArtifactRead, ArtifactRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `artifacts` table.
pub(crate) struct SqliteArtifactRepo {
    conn: SharedConn,
}

impl SqliteArtifactRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

const SELECT_ARTIFACT: &str = "SELECT artifact_id, uri, digest, media_type, size_bytes, \
     origin_run_id, origin_effect_id, sensitivity, retention, locator, created_at_ms FROM artifacts";

fn decode_artifact(row: &SqliteRow) -> errors::Result<ArtifactRow> {
    Ok(ArtifactRow {
        artifact_id: mapping::decode_id(
            "artifacts.artifact_id",
            &mapping::text(row, "artifact_id")?,
        )?,
        uri: mapping::text(row, "uri")?,
        digest: mapping::text(row, "digest")?,
        media_type: mapping::text(row, "media_type")?,
        size_bytes: mapping::int(row, "size_bytes")?,
        origin_run_id: mapping::decode_id(
            "artifacts.origin_run_id",
            &mapping::text(row, "origin_run_id")?,
        )?,
        origin_effect_id: mapping::decode_opt_id(
            "artifacts.origin_effect_id",
            mapping::opt_text(row, "origin_effect_id")?,
        )?,
        sensitivity: mapping::decode_wire(
            "artifacts.sensitivity",
            mapping::int(row, "sensitivity")?,
            SensitivityClass::from_wire,
        )?,
        retention: mapping::decode_wire(
            "artifacts.retention",
            mapping::int(row, "retention")?,
            RetentionClass::from_wire,
        )?,
        locator: mapping::text(row, "locator")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

#[async_trait]
impl ArtifactRead for SqliteArtifactRepo {
    async fn get_by_id(&mut self, id: ArtifactId) -> errors::Result<Option<ArtifactRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_ARTIFACT} WHERE artifact_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_artifact).transpose()
    }

    async fn get_by_uri(&mut self, uri: &str) -> errors::Result<Option<ArtifactRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_ARTIFACT} WHERE uri = ?1");
        let row = sqlx::query(&query)
            .bind(uri)
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_artifact).transpose()
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<ArtifactRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_ARTIFACT} WHERE origin_run_id = ?1 ORDER BY created_at_ms");
        let rows = sqlx::query(&query)
            .bind(run.to_string())
            .fetch_all(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_artifact).collect()
    }
}

#[async_trait]
impl ArtifactRepo for SqliteArtifactRepo {
    async fn insert(&mut self, artifact: NewArtifact) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO artifacts (artifact_id, uri, digest, media_type, size_bytes, \
             origin_run_id, origin_effect_id, sensitivity, retention, locator, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(artifact.artifact_id.to_string())
        .bind(artifact.uri)
        .bind(artifact.digest)
        .bind(artifact.media_type)
        .bind(artifact.size_bytes)
        .bind(artifact.origin_run_id.to_string())
        .bind(artifact.origin_effect_id.map(|id| id.to_string()))
        .bind(i64::from(artifact.sensitivity.to_wire()))
        .bind(i64::from(artifact.retention.to_wire()))
        .bind(artifact.locator)
        .bind(artifact.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
