//! Atomic effect claims (EFF-003).
//!
//! The claim predicate — `state = Prepared OR (state = Claimed AND lease
//! expired)` — cannot be expressed by [`EffectRepo::cas_transition`], so it
//! lives in a single conditional `UPDATE` here. On a hit the row is stamped
//! with the executor identity, the current daemon fencing epoch, the new
//! lease expiry, and a monotonically increasing `executor_fencing_token`
//! (`COALESCE(token, 0) + 1`), all atomically inside the write transaction
//! (R3.2, executor leases in `effect-coordinator.md`).

use domain::effect::EffectState;
use domain::ids::EffectId;
use sqlx::Row;

use crate::mapping;
use crate::repos::effects::SqliteEffectRepo;

impl SqliteEffectRepo {
    /// Executes the atomic claim; `Ok(Some(token))` on a successful claim.
    pub(crate) async fn claim(
        &self,
        id: EffectId,
        executor_id: &str,
        daemon_epoch: u64,
        lease_expires_ms: i64,
        now_ms: i64,
    ) -> errors::Result<Option<u64>> {
        let mut guard = self.conn.lock().await;
        let daemon_epoch = mapping::encode_u64("effects.daemon_fencing_epoch", daemon_epoch)?;
        let result = sqlx::query(
            "UPDATE effects SET \
             state = ?1, \
             executor_id = ?2, \
             executor_fencing_token = COALESCE(executor_fencing_token, 0) + 1, \
             daemon_fencing_epoch = ?3, \
             lease_expires_ms = ?4, \
             updated_at_ms = ?5 \
             WHERE effect_id = ?6 \
             AND (state = ?7 OR (state = ?8 AND lease_expires_ms <= ?5)) \
             RETURNING executor_fencing_token",
        )
        .bind(i64::from(EffectState::Claimed.to_wire()))
        .bind(executor_id)
        .bind(daemon_epoch)
        .bind(lease_expires_ms)
        .bind(now_ms)
        .bind(id.to_string())
        .bind(i64::from(EffectState::Prepared.to_wire()))
        .bind(i64::from(EffectState::Claimed.to_wire()))
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        drop(guard);
        result
            .map(|row| mapping::decode_u64("effects.executor_fencing_token", row.get::<i64, _>(0)))
            .transpose()
    }
}
