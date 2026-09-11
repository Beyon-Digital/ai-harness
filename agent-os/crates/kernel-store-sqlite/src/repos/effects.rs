//! Effect repository: insert, fetch, list, and compare-and-set transitions.
//!
//! A transition checks the persisted state and, when present, the executor
//! fencing token; the patch is applied atomically and a stale expectation
//! reports `false` without mutation (R3.2).

use async_trait::async_trait;
use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::ids::{EffectId, RunId};
use kernel_store::models::{EffectPatch, EffectRow, NewEffect};
use kernel_store::repositories::{EffectRead, EffectRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the `effects` table.
pub(crate) struct SqliteEffectRepo {
    conn: SharedConn,
}

impl SqliteEffectRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

const SELECT_EFFECT: &str = "SELECT effect_id, run_id, step_sequence, decision_id, operation, \
     request_hash, request_payload, effect_class, idempotency_semantics, \
     reconciliation_semantics, cancellation_semantics, compensation_capability, adapter_id, \
     adapter_version, adapter_digest, state, executor_id, executor_fencing_token, \
     daemon_fencing_epoch, lease_expires_ms, provider_operation_ref, result_ref, error_code, \
     created_at_ms, updated_at_ms FROM effects";

fn decode_effect(row: &SqliteRow) -> errors::Result<EffectRow> {
    Ok(EffectRow {
        effect_id: mapping::decode_id("effects.effect_id", &mapping::text(row, "effect_id")?)?,
        run_id: mapping::decode_id("effects.run_id", &mapping::text(row, "run_id")?)?,
        step_sequence: mapping::decode_u64(
            "effects.step_sequence",
            mapping::int(row, "step_sequence")?,
        )?,
        decision_id: mapping::decode_id(
            "effects.decision_id",
            &mapping::text(row, "decision_id")?,
        )?,
        operation: mapping::text(row, "operation")?,
        request_hash: mapping::text(row, "request_hash")?,
        request_payload: mapping::blob(row, "request_payload")?,
        effect_class: mapping::decode_wire(
            "effects.effect_class",
            mapping::int(row, "effect_class")?,
            EffectClass::from_wire,
        )?,
        idempotency_semantics: mapping::decode_wire(
            "effects.idempotency_semantics",
            mapping::int(row, "idempotency_semantics")?,
            IdempotencySemantics::from_wire,
        )?,
        reconciliation_semantics: mapping::decode_wire(
            "effects.reconciliation_semantics",
            mapping::int(row, "reconciliation_semantics")?,
            ReconciliationSemantics::from_wire,
        )?,
        cancellation_semantics: mapping::text(row, "cancellation_semantics")?,
        compensation_capability: mapping::opt_text(row, "compensation_capability")?,
        adapter_id: mapping::decode_id("effects.adapter_id", &mapping::text(row, "adapter_id")?)?,
        adapter_version: mapping::text(row, "adapter_version")?,
        adapter_digest: mapping::text(row, "adapter_digest")?,
        state: mapping::decode_wire(
            "effects.state",
            mapping::int(row, "state")?,
            EffectState::from_wire,
        )?,
        executor_id: mapping::opt_text(row, "executor_id")?,
        executor_fencing_token: mapping::opt_int(row, "executor_fencing_token")?
            .map(|value| mapping::decode_u64("effects.executor_fencing_token", value))
            .transpose()?,
        daemon_fencing_epoch: mapping::opt_int(row, "daemon_fencing_epoch")?
            .map(|value| mapping::decode_u64("effects.daemon_fencing_epoch", value))
            .transpose()?,
        lease_expires_ms: mapping::opt_int(row, "lease_expires_ms")?,
        provider_operation_ref: mapping::opt_text(row, "provider_operation_ref")?,
        result_ref: mapping::opt_text(row, "result_ref")?,
        error_code: mapping::opt_text(row, "error_code")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        updated_at_ms: mapping::int(row, "updated_at_ms")?,
    })
}

async fn fetch_effect(
    conn: &mut sqlx::SqliteConnection,
    id: EffectId,
) -> errors::Result<Option<EffectRow>> {
    let query = format!("{SELECT_EFFECT} WHERE effect_id = ?1");
    let row = sqlx::query(&query)
        .bind(id.to_string())
        .fetch_optional(conn)
        .await
        .map_err(mapping::from_sqlx)?;
    row.as_ref().map(decode_effect).transpose()
}

#[async_trait]
impl EffectRead for SqliteEffectRepo {
    async fn get(&mut self, id: EffectId) -> errors::Result<Option<EffectRow>> {
        let mut guard = self.conn.lock().await;
        fetch_effect(guard.connection()?, id).await
    }

    async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<EffectRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!("{SELECT_EFFECT} WHERE run_id = ?1 ORDER BY step_sequence, effect_id");
        let rows = sqlx::query(&query)
            .bind(run.to_string())
            .fetch_all(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_effect).collect()
    }
}

#[async_trait]
impl EffectRepo for SqliteEffectRepo {
    async fn insert(&mut self, effect: NewEffect) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO effects (effect_id, run_id, step_sequence, decision_id, operation, \
             request_hash, request_payload, effect_class, idempotency_semantics, \
             reconciliation_semantics, cancellation_semantics, compensation_capability, \
             adapter_id, adapter_version, adapter_digest, state, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
             ?17, ?17)",
        )
        .bind(effect.effect_id.to_string())
        .bind(effect.run_id.to_string())
        .bind(mapping::encode_u64(
            "effects.step_sequence",
            effect.step_sequence,
        )?)
        .bind(effect.decision_id.to_string())
        .bind(effect.operation)
        .bind(effect.request_hash)
        .bind(effect.request_payload)
        .bind(i64::from(effect.effect_class.to_wire()))
        .bind(i64::from(effect.idempotency_semantics.to_wire()))
        .bind(i64::from(effect.reconciliation_semantics.to_wire()))
        .bind(effect.cancellation_semantics)
        .bind(effect.compensation_capability)
        .bind(effect.adapter_id.to_string())
        .bind(effect.adapter_version)
        .bind(effect.adapter_digest)
        .bind(i64::from(effect.state.to_wire()))
        .bind(effect.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_transition(
        &mut self,
        id: EffectId,
        expect_state: EffectState,
        expect_token: Option<u64>,
        patch: EffectPatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let expected_token = expect_token
            .map(|value| mapping::encode_u64("effects.executor_fencing_token", value))
            .transpose()?;
        let patch_token = patch
            .executor_fencing_token
            .map(|value| mapping::encode_u64("effects.executor_fencing_token", value))
            .transpose()?;
        let daemon_epoch = patch
            .daemon_fencing_epoch
            .map(|value| mapping::encode_u64("effects.daemon_fencing_epoch", value))
            .transpose()?;
        let result = sqlx::query(
            "UPDATE effects SET \
             state = COALESCE(?1, state), \
             executor_id = COALESCE(?2, executor_id), \
             executor_fencing_token = COALESCE(?3, executor_fencing_token), \
             daemon_fencing_epoch = COALESCE(?4, daemon_fencing_epoch), \
             lease_expires_ms = COALESCE(?5, lease_expires_ms), \
             provider_operation_ref = COALESCE(?6, provider_operation_ref), \
             result_ref = COALESCE(?7, result_ref), \
             error_code = COALESCE(?8, error_code) \
             WHERE effect_id = ?9 AND state = ?10 \
             AND (?11 IS NULL OR executor_fencing_token = ?11)",
        )
        .bind(patch.state.map(|state| i64::from(state.to_wire())))
        .bind(patch.executor_id)
        .bind(patch_token)
        .bind(daemon_epoch)
        .bind(patch.lease_expires_ms)
        .bind(patch.provider_operation_ref)
        .bind(patch.result_ref)
        .bind(patch.error_code)
        .bind(id.to_string())
        .bind(i64::from(expect_state.to_wire()))
        .bind(expected_token)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }
}
