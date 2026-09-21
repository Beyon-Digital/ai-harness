//! Run repository: insert, fetch, list, and compare-and-set updates.

use async_trait::async_trait;
use domain::ids::{RunId, TaskId};
use domain::run::{RecoveryDisposition, RunState};
use kernel_store::models::{NewRun, RunCas, RunPatch, RunRow};
use kernel_store::repositories::{RunRead, RunRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `runs` table.
pub(crate) struct SqliteRunRepo {
    conn: SharedConn,
}

impl SqliteRunRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

async fn fetch_run(conn: &mut sqlx::SqliteConnection, id: RunId) -> errors::Result<Option<RunRow>> {
    let row = sqlx::query(
        "SELECT run_id, task_id, session_id, parent_run_id, state, recovery_disposition, \
         run_revision, loop_epoch, step_sequence, input_event_cursor, cancellation_epoch, \
         resolved_environment_id, agent_spec_id, agent_spec_version, agent_spec_digest, \
         requested_profile, workspace_uri, claim_owner, claim_token, claim_expires_ms, \
         claim_daemon_epoch, terminal_reason, output_ref, current_turn_id, created_at_ms, \
         updated_at_ms FROM runs WHERE run_id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(conn)
    .await
    .map_err(mapping::from_sqlx)?;
    row.as_ref().map(decode_run).transpose()
}

async fn fetch_runs_by_task(
    conn: &mut sqlx::SqliteConnection,
    task: TaskId,
) -> errors::Result<Vec<RunRow>> {
    let rows = sqlx::query(
        "SELECT run_id, task_id, session_id, parent_run_id, state, recovery_disposition, \
         run_revision, loop_epoch, step_sequence, input_event_cursor, cancellation_epoch, \
         resolved_environment_id, agent_spec_id, agent_spec_version, agent_spec_digest, \
         requested_profile, workspace_uri, claim_owner, claim_token, claim_expires_ms, \
         claim_daemon_epoch, terminal_reason, output_ref, current_turn_id, created_at_ms, \
         updated_at_ms FROM runs WHERE task_id = ?1 ORDER BY run_id",
    )
    .bind(task.to_string())
    .fetch_all(conn)
    .await
    .map_err(mapping::from_sqlx)?;
    rows.iter().map(decode_run).collect()
}

async fn fetch_active_runs(
    conn: &mut sqlx::SqliteConnection,
    terminal_states: [i64; 3],
) -> errors::Result<Vec<RunRow>> {
    let rows = sqlx::query(
        "SELECT run_id, task_id, session_id, parent_run_id, state, recovery_disposition, \
         run_revision, loop_epoch, step_sequence, input_event_cursor, cancellation_epoch, \
         resolved_environment_id, agent_spec_id, agent_spec_version, agent_spec_digest, \
         requested_profile, workspace_uri, claim_owner, claim_token, claim_expires_ms, \
         claim_daemon_epoch, terminal_reason, output_ref, current_turn_id, created_at_ms, \
         updated_at_ms FROM runs WHERE state NOT IN (?1, ?2, ?3) ORDER BY created_at_ms, run_id",
    )
    .bind(terminal_states[0])
    .bind(terminal_states[1])
    .bind(terminal_states[2])
    .fetch_all(conn)
    .await
    .map_err(mapping::from_sqlx)?;
    rows.iter().map(decode_run).collect()
}

fn decode_run(row: &SqliteRow) -> errors::Result<RunRow> {
    Ok(RunRow {
        run_id: mapping::decode_id("runs.run_id", &mapping::text(row, "run_id")?)?,
        task_id: mapping::decode_id("runs.task_id", &mapping::text(row, "task_id")?)?,
        session_id: mapping::decode_opt_id(
            "runs.session_id",
            mapping::opt_text(row, "session_id")?,
        )?,
        parent_run_id: mapping::decode_opt_id(
            "runs.parent_run_id",
            mapping::opt_text(row, "parent_run_id")?,
        )?,
        state: mapping::decode_wire(
            "runs.state",
            mapping::int(row, "state")?,
            RunState::from_wire,
        )?,
        recovery: mapping::decode_wire(
            "runs.recovery_disposition",
            mapping::int(row, "recovery_disposition")?,
            RecoveryDisposition::from_wire,
        )?,
        run_revision: mapping::decode_u64("runs.run_revision", mapping::int(row, "run_revision")?)?,
        loop_epoch: mapping::decode_u64("runs.loop_epoch", mapping::int(row, "loop_epoch")?)?,
        step_sequence: mapping::decode_u64(
            "runs.step_sequence",
            mapping::int(row, "step_sequence")?,
        )?,
        input_event_cursor: mapping::decode_id(
            "runs.input_event_cursor",
            &mapping::text(row, "input_event_cursor")?,
        )?,
        cancellation_epoch: mapping::decode_u64(
            "runs.cancellation_epoch",
            mapping::int(row, "cancellation_epoch")?,
        )?,
        resolved_environment_id: mapping::decode_opt_id(
            "runs.resolved_environment_id",
            mapping::opt_text(row, "resolved_environment_id")?,
        )?,
        agent_spec_id: mapping::decode_opt_id(
            "runs.agent_spec_id",
            mapping::opt_text(row, "agent_spec_id")?,
        )?,
        agent_spec_version: mapping::opt_text(row, "agent_spec_version")?,
        agent_spec_digest: mapping::opt_text(row, "agent_spec_digest")?,
        requested_profile: mapping::text(row, "requested_profile")?,
        workspace_uri: mapping::opt_text(row, "workspace_uri")?,
        claim_owner: mapping::opt_text(row, "claim_owner")?,
        claim_token: mapping::opt_int(row, "claim_token")?
            .map(|value| mapping::decode_u64("runs.claim_token", value))
            .transpose()?,
        claim_expires_ms: mapping::opt_int(row, "claim_expires_ms")?,
        claim_daemon_epoch: mapping::opt_int(row, "claim_daemon_epoch")?
            .map(|value| mapping::decode_u64("runs.claim_daemon_epoch", value))
            .transpose()?,
        terminal_reason: mapping::opt_text(row, "terminal_reason")?,
        output_ref: mapping::opt_text(row, "output_ref")?,
        current_turn_id: mapping::decode_opt_id(
            "runs.current_turn_id",
            mapping::opt_text(row, "current_turn_id")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        updated_at_ms: mapping::int(row, "updated_at_ms")?,
    })
}

#[async_trait]
impl RunRead for SqliteRunRepo {
    async fn get(&mut self, id: RunId) -> errors::Result<Option<RunRow>> {
        let mut guard = self.conn.lock().await;
        fetch_run(guard.connection()?, id).await
    }

    async fn list_by_task(&mut self, task: TaskId) -> errors::Result<Vec<RunRow>> {
        let mut guard = self.conn.lock().await;
        fetch_runs_by_task(guard.connection()?, task).await
    }

    async fn list_active(&mut self) -> errors::Result<Vec<RunRow>> {
        let terminal_states = [
            i64::from(RunState::Completed.to_wire()),
            i64::from(RunState::Failed.to_wire()),
            i64::from(RunState::Cancelled.to_wire()),
        ];
        let mut guard = self.conn.lock().await;
        fetch_active_runs(guard.connection()?, terminal_states).await
    }
}

#[async_trait]
impl RunRepo for SqliteRunRepo {
    async fn insert(&mut self, run: NewRun) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;
        sqlx::query(
            "INSERT INTO runs (run_id, task_id, session_id, parent_run_id, state, \
             recovery_disposition, loop_epoch, step_sequence, input_event_cursor, \
             cancellation_epoch, resolved_environment_id, agent_spec_id, \
             agent_spec_version, agent_spec_digest, requested_profile, \
             workspace_uri, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
             ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        )
        .bind(run.run_id.to_string())
        .bind(run.task_id.to_string())
        .bind(run.session_id.map(|id| id.to_string()))
        .bind(run.parent_run_id.map(|id| id.to_string()))
        .bind(i64::from(run.state.to_wire()))
        .bind(i64::from(run.recovery.to_wire()))
        .bind(mapping::encode_u64("runs.loop_epoch", run.loop_epoch)?)
        .bind(mapping::encode_u64(
            "runs.step_sequence",
            run.step_sequence,
        )?)
        .bind(run.input_event_cursor.to_string())
        .bind(mapping::encode_u64(
            "runs.cancellation_epoch",
            run.cancellation_epoch,
        )?)
        .bind(run.resolved_environment_id.map(|id| id.to_string()))
        .bind(run.agent_spec_id.map(|id| id.to_string()))
        .bind(run.agent_spec_version)
        .bind(run.agent_spec_digest)
        .bind(run.requested_profile)
        .bind(run.workspace_uri)
        .bind(run.created_at_ms)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_update(
        &mut self,
        id: RunId,
        expect: RunCas,
        patch: RunPatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;

        let (claim_owner, claim_token, claim_expires_ms, claim_daemon_epoch) = match patch.claim {
            Some(claim) => (
                Some(claim.owner),
                Some(mapping::encode_u64("runs.claim_token", claim.token)?),
                Some(claim.expires_unix_ms),
                Some(mapping::encode_u64(
                    "runs.claim_daemon_epoch",
                    claim.daemon_epoch,
                )?),
            ),
            None => (None, None, None, None),
        };
        let runtime = patch
            .step_sequence
            .map(|value| mapping::encode_u64("runs.step_sequence", value))
            .transpose()?;
        let loop_epoch = patch
            .loop_epoch
            .map(|value| mapping::encode_u64("runs.loop_epoch", value))
            .transpose()?;
        let cancellation_epoch = patch
            .cancellation_epoch
            .map(|value| mapping::encode_u64("runs.cancellation_epoch", value))
            .transpose()?;
        let expected_revision = mapping::encode_u64("runs.run_revision", expect.run_revision)?;
        let expected_state = expect.state.map(|state| i64::from(state.to_wire()));
        let expected_cancellation = expect
            .cancellation_epoch
            .map(|value| mapping::encode_u64("runs.cancellation_epoch", value))
            .transpose()?;

        let result = sqlx::query(
            "UPDATE runs SET \
             state = COALESCE(?1, state), \
             recovery_disposition = COALESCE(?2, recovery_disposition), \
             loop_epoch = COALESCE(?3, loop_epoch), \
             step_sequence = COALESCE(?4, step_sequence), \
             input_event_cursor = COALESCE(?5, input_event_cursor), \
             cancellation_epoch = COALESCE(?6, cancellation_epoch), \
             resolved_environment_id = COALESCE(?7, resolved_environment_id), \
             claim_owner = COALESCE(?8, claim_owner), \
             claim_token = COALESCE(?9, claim_token), \
             claim_expires_ms = COALESCE(?10, claim_expires_ms), \
             claim_daemon_epoch = COALESCE(?11, claim_daemon_epoch), \
             terminal_reason = COALESCE(?12, terminal_reason), \
             output_ref = COALESCE(?13, output_ref), \
             current_turn_id = COALESCE(?14, current_turn_id), \
             run_revision = run_revision + ?15 \
             WHERE run_id = ?16 AND run_revision = ?17 \
             AND (?18 IS NULL OR state = ?18) \
             AND (?19 IS NULL OR cancellation_epoch = ?19)",
        )
        .bind(patch.state.map(|state| i64::from(state.to_wire())))
        .bind(patch.recovery.map(|recovery| i64::from(recovery.to_wire())))
        .bind(loop_epoch)
        .bind(runtime)
        .bind(patch.input_event_cursor.map(|cursor| cursor.to_string()))
        .bind(cancellation_epoch)
        .bind(patch.resolved_environment_id.map(|id| id.to_string()))
        .bind(claim_owner)
        .bind(claim_token)
        .bind(claim_expires_ms)
        .bind(claim_daemon_epoch)
        .bind(patch.terminal_reason)
        .bind(patch.output_ref)
        .bind(patch.current_turn_id.map(|id| id.to_string()))
        .bind(i64::from(patch.bump_revision))
        .bind(id.to_string())
        .bind(expected_revision)
        .bind(expected_state)
        .bind(expected_cancellation)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
