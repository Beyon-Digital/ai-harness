//! Resource reservation repository: insert, fetch, list, and state CAS.

use async_trait::async_trait;
use domain::ids::{ReservationId, RunId};
use domain::resource::ReservationState;
use kernel_store::models::{NewReservation, ReservationPatch, ReservationRow};
use kernel_store::repositories::{ResourceRead, ResourceRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `resource_reservations` table.
pub(crate) struct SqliteResourceRepo {
    conn: SharedConn,
}

impl SqliteResourceRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

const SELECT_RESERVATION: &str = "SELECT reservation_id, run_id, resource_type, state, amount, \
     unit, fencing_token, parent_reservation_id, created_at_ms, updated_at_ms \
     FROM resource_reservations";

fn decode_reservation(row: &SqliteRow) -> errors::Result<ReservationRow> {
    Ok(ReservationRow {
        reservation_id: mapping::decode_id(
            "resource_reservations.reservation_id",
            &mapping::text(row, "reservation_id")?,
        )?,
        run_id: mapping::decode_id(
            "resource_reservations.run_id",
            &mapping::text(row, "run_id")?,
        )?,
        resource_type: mapping::text(row, "resource_type")?,
        state: mapping::decode_state(
            "resource_reservations.state",
            &mapping::text(row, "state")?,
            ReservationState::from_state_str,
        )?,
        amount: mapping::int(row, "amount")?,
        unit: mapping::text(row, "unit")?,
        fencing_token: mapping::decode_u64(
            "resource_reservations.fencing_token",
            mapping::int(row, "fencing_token")?,
        )?,
        parent_reservation_id: mapping::decode_opt_id(
            "resource_reservations.parent_reservation_id",
            mapping::opt_text(row, "parent_reservation_id")?,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        updated_at_ms: mapping::int(row, "updated_at_ms")?,
    })
}

#[async_trait]
impl ResourceRead for SqliteResourceRepo {
    async fn get(&mut self, id: ReservationId) -> errors::Result<Option<ReservationRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_RESERVATION} WHERE reservation_id = ?1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_reservation).transpose()
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<ReservationRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_RESERVATION} WHERE run_id = ?1 ORDER BY reservation_id");
        let rows = sqlx::query(&query)
            .bind(run.to_string())
            .fetch_all(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_reservation).collect()
    }

    async fn list_children(
        &mut self,
        parent: ReservationId,
    ) -> errors::Result<Vec<ReservationRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!(
            "{SELECT_RESERVATION} WHERE parent_reservation_id = ?1 ORDER BY reservation_id"
        );
        let rows = sqlx::query(&query)
            .bind(parent.to_string())
            .fetch_all(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_reservation).collect()
    }
}

#[async_trait]
impl ResourceRepo for SqliteResourceRepo {
    async fn insert(&mut self, reservation: NewReservation) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO resource_reservations (reservation_id, run_id, resource_type, state, \
             amount, unit, fencing_token, parent_reservation_id, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        )
        .bind(reservation.reservation_id.to_string())
        .bind(reservation.run_id.to_string())
        .bind(reservation.resource_type)
        .bind(reservation.state.as_str())
        .bind(reservation.amount)
        .bind(reservation.unit)
        .bind(mapping::encode_u64(
            "resource_reservations.fencing_token",
            reservation.fencing_token,
        )?)
        .bind(reservation.parent_reservation_id.map(|id| id.to_string()))
        .bind(reservation.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_transition(
        &mut self,
        id: ReservationId,
        expect_state: ReservationState,
        patch: ReservationPatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let fencing_token = patch
            .fencing_token
            .map(|value| mapping::encode_u64("resource_reservations.fencing_token", value))
            .transpose()?;
        let result = sqlx::query(
            "UPDATE resource_reservations SET \
             state = COALESCE(?1, state), \
             fencing_token = COALESCE(?2, fencing_token) \
             WHERE reservation_id = ?3 AND state = ?4",
        )
        .bind(patch.state.map(|state| state.as_str()))
        .bind(fencing_token)
        .bind(id.to_string())
        .bind(expect_state.as_str())
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
