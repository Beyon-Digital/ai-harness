//! Run-graph repository: heads, dependency edges, and reachability.
//!
//! Heads and dependency insertion reserve writer ordering through the
//! enclosing `BEGIN IMMEDIATE` transaction (D2, R3.3): the expected revision
//! is validated and the head advanced in the same immediate transaction.

use async_trait::async_trait;
use domain::ids::{RunId, TaskId};
use domain::resource::DependencyCondition;
use domain::run::UnknownStateValue;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewRunDependency, RunDependencyRow, RunGraphHeadRow};
use kernel_store::repositories::{GraphRead, GraphRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `run_graph_heads` and `run_dependencies`.
pub(crate) struct SqliteGraphRepo {
    conn: SharedConn,
}

impl SqliteGraphRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_head(row: &SqliteRow) -> errors::Result<RunGraphHeadRow> {
    Ok(RunGraphHeadRow {
        task_id: mapping::decode_id("run_graph_heads.task_id", &mapping::text(row, "task_id")?)?,
        graph_revision: mapping::decode_u64(
            "run_graph_heads.graph_revision",
            mapping::int(row, "graph_revision")?,
        )?,
    })
}

/// Parses the exact `run_dependencies.dependency_condition` CHECK literal.
fn dependency_condition_from_state(value: &str) -> Result<DependencyCondition, UnknownStateValue> {
    match value {
        "completed_successfully" => Ok(DependencyCondition::CompletedSuccessfully),
        "any_terminal" => Ok(DependencyCondition::AnyTerminal),
        "completed_or_cancelled" => Ok(DependencyCondition::CompletedOrCancelled),
        _ => Err(UnknownStateValue {
            value: value.to_owned(),
            enum_name: "DependencyCondition",
        }),
    }
}

/// Renders the exact `run_dependencies.dependency_condition` CHECK literal.
fn dependency_condition_to_state(condition: DependencyCondition) -> &'static str {
    match condition {
        DependencyCondition::CompletedSuccessfully => "completed_successfully",
        DependencyCondition::AnyTerminal => "any_terminal",
        DependencyCondition::CompletedOrCancelled => "completed_or_cancelled",
        DependencyCondition::Unspecified => "unspecified",
    }
}

fn decode_dependency(row: &SqliteRow) -> errors::Result<RunDependencyRow> {
    Ok(RunDependencyRow {
        dependency_id: mapping::decode_id(
            "run_dependencies.dependency_id",
            &mapping::text(row, "dependency_id")?,
        )?,
        task_id: mapping::decode_id("run_dependencies.task_id", &mapping::text(row, "task_id")?)?,
        source_run_id: mapping::decode_id(
            "run_dependencies.source_run_id",
            &mapping::text(row, "source_run_id")?,
        )?,
        target_run_id: mapping::decode_id(
            "run_dependencies.target_run_id",
            &mapping::text(row, "target_run_id")?,
        )?,
        dependency_condition: mapping::decode_state(
            "run_dependencies.dependency_condition",
            &mapping::text(row, "dependency_condition")?,
            dependency_condition_from_state,
        )?,
        created_graph_revision: mapping::decode_u64(
            "run_dependencies.created_graph_revision",
            mapping::int(row, "created_graph_revision")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

async fn fetch_head(
    conn: &mut sqlx::SqliteConnection,
    task: TaskId,
) -> errors::Result<Option<RunGraphHeadRow>> {
    let row = sqlx::query("SELECT task_id, graph_revision FROM run_graph_heads WHERE task_id = ?1")
        .bind(task.to_string())
        .fetch_optional(conn)
        .await
        .map_err(mapping::from_sqlx)?;
    row.as_ref().map(decode_head).transpose()
}

#[async_trait]
impl GraphRead for SqliteGraphRepo {
    async fn get_head(&mut self, task: TaskId) -> errors::Result<Option<RunGraphHeadRow>> {
        let mut guard = self.conn.lock().await;
        fetch_head(guard.connection()?, task).await
    }

    async fn list_dependencies(&mut self, task: TaskId) -> errors::Result<Vec<RunDependencyRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT dependency_id, task_id, source_run_id, target_run_id, \
             dependency_condition, created_graph_revision, created_at_ms \
             FROM run_dependencies WHERE task_id = ?1 \
             ORDER BY created_graph_revision, dependency_id",
        )
        .bind(task.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_dependency).collect()
    }

    async fn is_reachable(&mut self, from: RunId, to: RunId) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let reachable: i64 = sqlx::query_scalar(
            "WITH RECURSIVE reach(run_id) AS ( \
               SELECT ?1 \
               UNION \
               SELECT dependency.target_run_id FROM run_dependencies dependency \
               JOIN reach ON dependency.source_run_id = reach.run_id \
             ) SELECT EXISTS(SELECT 1 FROM reach WHERE run_id = ?2)",
        )
        .bind(from.to_string())
        .bind(to.to_string())
        .fetch_one(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(reachable != 0)
    }
}

#[async_trait]
impl GraphRepo for SqliteGraphRepo {
    async fn ensure_head(&mut self, task: TaskId) -> errors::Result<RunGraphHeadRow> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        sqlx::query(
            "INSERT INTO run_graph_heads (task_id, graph_revision) VALUES (?1, 0) \
             ON CONFLICT(task_id) DO NOTHING",
        )
        .bind(task.to_string())
        .execute(&mut *conn)
        .await
        .map_err(mapping::from_sqlx)?;
        fetch_head(conn, task)
            .await?
            .ok_or_else(|| missing_row("graph head is missing after ensure_head"))
    }

    async fn cas_head_revision(&mut self, task: TaskId, expected: u64) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let expected = mapping::encode_u64("run_graph_heads.graph_revision", expected)?;
        let result = sqlx::query(
            "UPDATE run_graph_heads SET graph_revision = graph_revision + 1 \
             WHERE task_id = ?1 AND graph_revision = ?2",
        )
        .bind(task.to_string())
        .bind(expected)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_dependency(
        &mut self,
        new: NewRunDependency,
        expected_revision: u64,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;

        let head = fetch_head(conn, new.task_id).await?;
        let Some(head) = head else {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "graph head missing for task",
            ));
        };
        if head.graph_revision != expected_revision {
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "graph revision conflict",
            ));
        }

        sqlx::query(
            "INSERT INTO run_dependencies (dependency_id, task_id, source_run_id, target_run_id, \
             dependency_condition, created_graph_revision, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(new.dependency_id.to_string())
        .bind(new.task_id.to_string())
        .bind(new.source_run_id.to_string())
        .bind(new.target_run_id.to_string())
        .bind(dependency_condition_to_state(new.dependency_condition))
        .bind(mapping::encode_u64(
            "run_dependencies.created_graph_revision",
            expected_revision,
        )?)
        .bind(new.created_at_ms)
        .execute(&mut *conn)
        .await
        .map_err(mapping::from_sqlx)?;

        let advanced = sqlx::query(
            "UPDATE run_graph_heads SET graph_revision = graph_revision + 1 \
             WHERE task_id = ?1 AND graph_revision = ?2",
        )
        .bind(new.task_id.to_string())
        .bind(mapping::encode_u64(
            "run_graph_heads.graph_revision",
            expected_revision,
        )?)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        if advanced.rows_affected() != 1 {
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "graph head changed during dependency insertion",
            ));
        }
        Ok(())
    }
}

fn missing_row(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::NotFound, RetryClass::Never, message)
}
