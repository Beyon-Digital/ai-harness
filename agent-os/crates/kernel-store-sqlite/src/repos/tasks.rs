//! Task repository: insert, fetch, and list by session.

use async_trait::async_trait;
use domain::ids::{SessionId, TaskId};
use kernel_store::models::{NewTask, TaskRow};
use kernel_store::repositories::{TaskRead, TaskRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `tasks` table.
pub(crate) struct SqliteTaskRepo {
    conn: SharedConn,
}

impl SqliteTaskRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_task(row: &SqliteRow) -> errors::Result<TaskRow> {
    Ok(TaskRow {
        task_id: mapping::decode_id("tasks.task_id", &mapping::text(row, "task_id")?)?,
        session_id: mapping::decode_opt_id(
            "tasks.session_id",
            mapping::opt_text(row, "session_id")?,
        )?,
        created_by_actor_id: mapping::decode_id(
            "tasks.created_by_actor_id",
            &mapping::text(row, "created_by_actor_id")?,
        )?,
        task_kind: mapping::text(row, "task_kind")?,
        payload: mapping::blob(row, "payload")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

#[async_trait]
impl TaskRead for SqliteTaskRepo {
    async fn get(&mut self, id: TaskId) -> errors::Result<Option<TaskRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT task_id, session_id, created_by_actor_id, task_kind, payload, created_at_ms \
             FROM tasks WHERE task_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_task).transpose()
    }

    async fn list_by_session(&mut self, session: SessionId) -> errors::Result<Vec<TaskRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT task_id, session_id, created_by_actor_id, task_kind, payload, created_at_ms \
             FROM tasks WHERE session_id = ?1 ORDER BY task_id",
        )
        .bind(session.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_task).collect()
    }
}

#[async_trait]
impl TaskRepo for SqliteTaskRepo {
    async fn insert(&mut self, task: NewTask) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO tasks (task_id, session_id, created_by_actor_id, task_kind, payload, \
             created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(task.task_id.to_string())
        .bind(task.session_id.map(|id| id.to_string()))
        .bind(task.created_by_actor_id.to_string())
        .bind(task.task_kind)
        .bind(task.payload)
        .bind(task.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
