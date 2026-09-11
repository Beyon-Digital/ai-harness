//! Loop repository: turns, turn-state CAS, and accepted decisions.
//!
//! A turn transition checks the persisted state and applies the patch
//! atomically; a stale expectation reports `false` without mutation (R3.2).

use async_trait::async_trait;
use domain::ids::{DecisionId, RunId, TurnId};
use kernel_store::models::{DecisionRow, LoopTurnPatch, LoopTurnRow, NewDecision, NewLoopTurn};
use kernel_store::repositories::{LoopRead, LoopRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over `loop_turns` and `decisions`.
pub(crate) struct SqliteLoopRepo {
    conn: SharedConn,
}

impl SqliteLoopRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

const SELECT_TURN: &str = "SELECT turn_id, run_id, run_revision, loop_epoch, step_sequence, \
     input_event_cursor, state, issued_at_ms FROM loop_turns";

fn decode_turn(row: &SqliteRow) -> errors::Result<LoopTurnRow> {
    Ok(LoopTurnRow {
        turn_id: mapping::decode_id("loop_turns.turn_id", &mapping::text(row, "turn_id")?)?,
        run_id: mapping::decode_id("loop_turns.run_id", &mapping::text(row, "run_id")?)?,
        run_revision: mapping::decode_u64(
            "loop_turns.run_revision",
            mapping::int(row, "run_revision")?,
        )?,
        loop_epoch: mapping::decode_u64("loop_turns.loop_epoch", mapping::int(row, "loop_epoch")?)?,
        step_sequence: mapping::decode_u64(
            "loop_turns.step_sequence",
            mapping::int(row, "step_sequence")?,
        )?,
        input_event_cursor: mapping::decode_id(
            "loop_turns.input_event_cursor",
            &mapping::text(row, "input_event_cursor")?,
        )?,
        state: mapping::text(row, "state")?,
        issued_at_ms: mapping::int(row, "issued_at_ms")?,
    })
}

fn decode_decision(row: &SqliteRow) -> errors::Result<DecisionRow> {
    Ok(DecisionRow {
        decision_id: mapping::decode_id(
            "decisions.decision_id",
            &mapping::text(row, "decision_id")?,
        )?,
        run_id: mapping::decode_id("decisions.run_id", &mapping::text(row, "run_id")?)?,
        turn_id: mapping::decode_id("decisions.turn_id", &mapping::text(row, "turn_id")?)?,
        decision_type: mapping::text(row, "decision_type")?,
        decision_digest: mapping::text(row, "decision_digest")?,
        decision_bytes: mapping::blob(row, "decision_bytes")?,
        run_revision: mapping::decode_u64(
            "decisions.run_revision",
            mapping::int(row, "run_revision")?,
        )?,
        loop_epoch: mapping::decode_u64("decisions.loop_epoch", mapping::int(row, "loop_epoch")?)?,
        step_sequence: mapping::decode_u64(
            "decisions.step_sequence",
            mapping::int(row, "step_sequence")?,
        )?,
        input_event_cursor: mapping::decode_id(
            "decisions.input_event_cursor",
            &mapping::text(row, "input_event_cursor")?,
        )?,
        accepted_at_ms: mapping::int(row, "accepted_at_ms")?,
    })
}

#[async_trait]
impl LoopRead for SqliteLoopRepo {
    async fn get_turn(&mut self, id: TurnId) -> errors::Result<Option<LoopTurnRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_TURN} WHERE turn_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_turn).transpose()
    }

    async fn get_decision(
        &mut self,
        run: RunId,
        decision: DecisionId,
    ) -> errors::Result<Option<DecisionRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT decision_id, run_id, turn_id, decision_type, decision_digest, \
             decision_bytes, run_revision, loop_epoch, step_sequence, input_event_cursor, \
             accepted_at_ms FROM decisions WHERE run_id = ?1 AND decision_id = ?2",
        )
        .bind(run.to_string())
        .bind(decision.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_decision).transpose()
    }
}

#[async_trait]
impl LoopRepo for SqliteLoopRepo {
    async fn insert_turn(&mut self, turn: NewLoopTurn) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO loop_turns (turn_id, run_id, run_revision, loop_epoch, step_sequence, \
             input_event_cursor, state, issued_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(turn.turn_id.to_string())
        .bind(turn.run_id.to_string())
        .bind(mapping::encode_u64(
            "loop_turns.run_revision",
            turn.run_revision,
        )?)
        .bind(mapping::encode_u64(
            "loop_turns.loop_epoch",
            turn.loop_epoch,
        )?)
        .bind(mapping::encode_u64(
            "loop_turns.step_sequence",
            turn.step_sequence,
        )?)
        .bind(turn.input_event_cursor.to_string())
        .bind(turn.state)
        .bind(turn.issued_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_turn(
        &mut self,
        id: TurnId,
        expect_state: &str,
        patch: LoopTurnPatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let result = sqlx::query(
            "UPDATE loop_turns SET state = COALESCE(?1, state) \
             WHERE turn_id = ?2 AND state = ?3",
        )
        .bind(patch.state)
        .bind(id.to_string())
        .bind(expect_state)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_decision(&mut self, decision: NewDecision) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO decisions (decision_id, run_id, turn_id, decision_type, decision_digest, \
             decision_bytes, run_revision, loop_epoch, step_sequence, input_event_cursor, \
             accepted_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(decision.decision_id.to_string())
        .bind(decision.run_id.to_string())
        .bind(decision.turn_id.to_string())
        .bind(decision.decision_type)
        .bind(decision.decision_digest)
        .bind(decision.decision_bytes)
        .bind(mapping::encode_u64(
            "decisions.run_revision",
            decision.run_revision,
        )?)
        .bind(mapping::encode_u64(
            "decisions.loop_epoch",
            decision.loop_epoch,
        )?)
        .bind(mapping::encode_u64(
            "decisions.step_sequence",
            decision.step_sequence,
        )?)
        .bind(decision.input_event_cursor.to_string())
        .bind(decision.accepted_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
