//! Effect preparation: persist the immutable `EffectRecord` atomically inside
//! the owning command/loop-decision transaction.
//!
//! [`prepare_effect`] inserts the `Prepared` record, deduplicates on the
//! logical identity `(run_id, decision_id, operation, request_hash)`, and
//! stages `EffectPrepared`. Because the insert runs inside the caller's
//! `KernelTxn`, a rolled-back command leaves no prepared effect behind —
//! nothing can dispatch without a committed `Prepared` row.

use domain::effect::EffectState;
use domain::generated::contract;
use domain::ids::{AdapterId, DecisionId, EffectId, EventId, RunId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{EffectRow, NewEffect};
use prost::Message;
use sha2::{Digest, Sha256};

use crate::contract::EffectContract;
use crate::env::EffectEnv;
use crate::stage::stage_effect_event;

/// The frozen adapter identity an effect is bound to at prepare time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterBinding {
    /// Registered adapter identifier.
    pub adapter_id: AdapterId,
    /// Exact adapter version from the frozen run binding.
    pub adapter_version: String,
    /// Content digest of the adapter from the frozen run binding.
    pub adapter_digest: String,
}

/// Everything preparation needs, captured at decision time.
#[derive(Clone, Debug)]
pub struct PrepareRequest {
    /// Caller-allocated effect identity.
    pub effect_id: EffectId,
    /// Owning run.
    pub run_id: RunId,
    /// Loop step sequence that produced the decision.
    pub step_sequence: u64,
    /// Decision identity within the owning command.
    pub decision_id: DecisionId,
    /// Operation name dispatched to the adapter.
    pub operation: String,
    /// Canonical request bytes; hashed into the effect identity.
    pub request_payload: Vec<u8>,
    /// Frozen adapter binding from the resolved run environment.
    pub adapter: AdapterBinding,
    /// Kernel-effective contract produced by [`crate::policy::resolve`].
    pub contract: EffectContract,
}

/// Result of [`prepare_effect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// A new `Prepared` record was inserted and `EffectPrepared` staged.
    Prepared(EffectRow),
    /// The logical identity already exists; the persisted row is returned
    /// unchanged and no event is re-staged.
    Replayed(EffectRow),
}

/// Persists a `Prepared` effect inside `txn`.
///
/// A duplicate logical identity returns [`PrepareOutcome::Replayed`] with the
/// committed row — the retry never creates a second operation identity.
pub async fn prepare_effect(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    request: PrepareRequest,
) -> errors::Result<PrepareOutcome> {
    let request_hash = request_hash(&request.request_payload);
    let insert = txn
        .effects()
        .insert(NewEffect {
            effect_id: request.effect_id,
            run_id: request.run_id,
            step_sequence: request.step_sequence,
            decision_id: request.decision_id,
            operation: request.operation.clone(),
            request_hash: request_hash.clone(),
            request_payload: request.request_payload.clone(),
            effect_class: request.contract.effect_class,
            idempotency_semantics: request.contract.idempotency,
            reconciliation_semantics: request.contract.reconciliation,
            cancellation_semantics: request.contract.cancellation.as_str().to_owned(),
            compensation_capability: request.contract.compensation_capability.clone(),
            adapter_id: request.adapter.adapter_id,
            adapter_version: request.adapter.adapter_version.clone(),
            adapter_digest: request.adapter.adapter_digest.clone(),
            state: EffectState::Prepared,
            created_at_ms: env.clock.now_unix_ms(),
        })
        .await;
    if let Err(error) = insert {
        if error.code() == ErrorCode::Conflict {
            return replay_existing(txn, &request, &request_hash).await;
        }
        return Err(error);
    }
    let row = txn
        .effects()
        .get(request.effect_id)
        .await?
        .ok_or_else(|| internal("prepared effect is not visible in its transaction"))?;
    stage_effect_event(
        txn,
        EventId::new(env.ids),
        "EffectPrepared",
        request.effect_id,
        effect_record(&row).encode_to_vec(),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(PrepareOutcome::Prepared(row))
}

/// Finds the already-persisted row for a duplicate logical identity.
async fn replay_existing(
    txn: &mut dyn KernelTxn,
    request: &PrepareRequest,
    request_hash: &str,
) -> errors::Result<PrepareOutcome> {
    let rows = txn.effects().list_by_run(request.run_id).await?;
    let existing = rows.into_iter().find(|row| {
        row.decision_id == request.decision_id
            && row.operation == request.operation
            && row.request_hash == request_hash
    });
    match existing {
        Some(row) => Ok(PrepareOutcome::Replayed(row)),
        None => Err(KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "effect identity conflict without a matching persisted row",
        )),
    }
}

/// SHA-256 request digest rendered as 64 lowercase hexadecimal characters,
/// matching the digest convention used across the kernel store.
fn request_hash(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    let mut rendered = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// Projects a persisted row onto the wire `EffectRecord` for event payloads.
pub(crate) fn effect_record(row: &EffectRow) -> contract::EffectRecord {
    contract::EffectRecord {
        effect_id: row.effect_id.to_string(),
        run_id: row.run_id.to_string(),
        step_sequence: row.step_sequence,
        decision_id: row.decision_id.to_string(),
        operation: row.operation.clone(),
        request_hash: row.request_hash.clone(),
        effective_contract: Some(
            EffectContract {
                effect_class: row.effect_class,
                idempotency: row.idempotency_semantics,
                reconciliation: row.reconciliation_semantics,
                cancellation: crate::contract::CancellationSemantics::from_state_str(
                    &row.cancellation_semantics,
                )
                .unwrap_or(crate::contract::CancellationSemantics::Unsupported),
                compensation_capability: row.compensation_capability.clone(),
            }
            .to_contract(),
        ),
        adapter_id: row.adapter_id.to_string(),
        adapter_version: row.adapter_version.clone(),
        adapter_digest: row.adapter_digest.clone(),
        state: row.state.to_wire(),
        executor_id: row.executor_id.clone().unwrap_or_default(),
        executor_fencing_token: row.executor_fencing_token.unwrap_or(0),
        lease_expires_unix_ms: row.lease_expires_ms.unwrap_or(0),
        result_ref: row.result_ref.clone().unwrap_or_default(),
        error_code: row.error_code.clone().unwrap_or_default(),
    }
}

fn internal(message: impl Into<std::borrow::Cow<'static, str>>) -> KernelError {
    KernelError::new(ErrorCode::Internal, RetryClass::Never, message)
}

#[cfg(test)]
mod tests {
    use super::request_hash;

    #[test]
    fn request_hash_is_64_lowercase_hex() {
        let hash = request_hash(b"payload");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(hash, request_hash(b"payload"));
        assert_ne!(hash, request_hash(b"other"));
    }
}
