//! Timer repository: insert, fetch, due listing, and state/version CAS.
//!
//! A transition checks `(state, version)` and advances `version` on success,
//! so a stale claim observes `false` without mutation (R3.2).

use async_trait::async_trait;
use domain::ids::TimerId;
use domain::resource::TimerState;
use kernel_store::models::{NewTimer, TimerPatch, TimerRow};
use kernel_store::repositories::{TimerRead, TimerRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `timers` table.
pub(crate) struct SqliteTimerRepo {
    conn: SharedConn,
}

impl SqliteTimerRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

const SELECT_TIMER: &str = "SELECT timer_id, run_id, timer_kind, payload, due_at_ms, state, \
     version, claim_owner, claim_fencing_token, claim_daemon_epoch, created_at_ms, updated_at_ms \
     FROM timers";

fn decode_timer(row: &SqliteRow) -> errors::Result<TimerRow> {
    Ok(TimerRow {
        timer_id: mapping::decode_id("timers.timer_id", &mapping::text(row, "timer_id")?)?,
        run_id: mapping::decode_opt_id("timers.run_id", mapping::opt_text(row, "run_id")?)?,
        timer_kind: mapping::text(row, "timer_kind")?,
        payload: mapping::blob(row, "payload")?,
        due_at_ms: mapping::int(row, "due_at_ms")?,
        state: mapping::decode_state(
            "timers.state",
            &mapping::text(row, "state")?,
            TimerState::from_state_str,
        )?,
        version: mapping::decode_u64("timers.version", mapping::int(row, "version")?)?,
        claim_owner: mapping::opt_text(row, "claim_owner")?,
        claim_fencing_token: mapping::opt_int(row, "claim_fencing_token")?
            .map(|value| mapping::decode_u64("timers.claim_fencing_token", value))
            .transpose()?,
        claim_daemon_epoch: mapping::opt_int(row, "claim_daemon_epoch")?
            .map(|value| mapping::decode_u64("timers.claim_daemon_epoch", value))
            .transpose()?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        updated_at_ms: mapping::int(row, "updated_at_ms")?,
    })
}

#[async_trait]
impl TimerRead for SqliteTimerRepo {
    async fn get(&mut self, id: TimerId) -> errors::Result<Option<TimerRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_TIMER} WHERE timer_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_timer).transpose()
    }

    async fn list_due(&mut self, due_before_ms: i64) -> errors::Result<Vec<TimerRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_TIMER} WHERE due_at_ms <= ?1 ORDER BY due_at_ms, timer_id");
        let rows = sqlx::query(&query)
            .bind(due_before_ms)
            .fetch_all(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_timer).collect()
    }
}

#[async_trait]
impl TimerRepo for SqliteTimerRepo {
    async fn insert(&mut self, timer: NewTimer) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO timers (timer_id, run_id, timer_kind, payload, due_at_ms, state, version, \
             created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        )
        .bind(timer.timer_id.to_string())
        .bind(timer.run_id.map(|id| id.to_string()))
        .bind(timer.timer_kind)
        .bind(timer.payload)
        .bind(timer.due_at_ms)
        .bind(timer.state.as_str())
        .bind(mapping::encode_u64("timers.version", timer.version)?)
        .bind(timer.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_transition(
        &mut self,
        id: TimerId,
        expect_state: TimerState,
        expect_version: u64,
        patch: TimerPatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let claim_token = patch
            .claim_fencing_token
            .map(|value| mapping::encode_u64("timers.claim_fencing_token", value))
            .transpose()?;
        let claim_epoch = patch
            .claim_daemon_epoch
            .map(|value| mapping::encode_u64("timers.claim_daemon_epoch", value))
            .transpose()?;
        let result = sqlx::query(
            "UPDATE timers SET \
             state = COALESCE(?1, state), \
             claim_owner = COALESCE(?2, claim_owner), \
             claim_fencing_token = COALESCE(?3, claim_fencing_token), \
             claim_daemon_epoch = COALESCE(?4, claim_daemon_epoch), \
             due_at_ms = COALESCE(?5, due_at_ms), \
             version = version + 1 \
             WHERE timer_id = ?6 AND state = ?7 AND version = ?8",
        )
        .bind(patch.state.map(|state| state.as_str()))
        .bind(patch.claim_owner)
        .bind(claim_token)
        .bind(claim_epoch)
        .bind(patch.due_at_ms)
        .bind(id.to_string())
        .bind(expect_state.as_str())
        .bind(mapping::encode_u64("timers.version", expect_version)?)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
