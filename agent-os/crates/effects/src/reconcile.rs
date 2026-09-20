//! Reconciliation and `Unknown`-state resolution.
//!
//! When a dispatched effect's response is lost, the kernel reconciles against
//! the adapter by the operation's *stable identity* — never by issuing a new
//! operation ID. If the adapter cannot prove an outcome, the effect moves to
//! `Unknown` and is never retried automatically; only the explicit
//! `ResolveUnknownEffect` command (capability + approval checked by the
//! handler) resolves it via [`apply_resolution`].

use async_trait::async_trait;
use domain::effect::EffectState;
use domain::ids::{EffectId, EventId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{EffectPatch, EffectRow};
use prost::Message;

use crate::contract::EffectContract;
use crate::coordinator::effect_record;
use crate::env::EffectEnv;
use crate::executor::{Transition, transition};
use crate::stage::stage_effect_event;

/// The adapter-side executor port for a bound effect operation.
///
/// `operation_id` is the stable kernel operation identity (the persisted
/// `provider_operation_ref` when one exists, else the `effect_id`); a retry
/// always reuses it and never mints a fresh identity.
#[async_trait]
pub trait EffectExecutor: Send + Sync {
    /// Executes a prepared operation. Implementations must not return
    /// [`ExecutionOutcome::Unknown`] unless the provider outcome is genuinely
    /// undecidable.
    async fn execute(&self, request: &ExecutionRequest) -> errors::Result<ExecutionOutcome>;

    /// Reconciliation probe: asks the provider for the outcome of a
    /// previously dispatched operation by its stable identity.
    async fn status(&self, operation_id: &str) -> errors::Result<ObservedOutcome>;

    /// Cooperative cancellation of a dispatched operation.
    async fn cancel(&self, operation_id: &str) -> errors::Result<()>;
}

/// An effect invocation dispatched to an [`EffectExecutor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRequest {
    /// Stable operation identity, unique per prepared effect.
    pub operation_id: String,
    /// Operation name.
    pub operation: String,
    /// Canonical request payload bytes.
    pub payload: Vec<u8>,
}

/// Terminal or undecidable outcome of an [`EffectExecutor::execute`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// The operation completed; `result_ref` names the stored result.
    Acknowledged { result_ref: String },
    /// The operation failed terminally on the adapter side.
    Failed { error_code: String },
    /// The response was lost; the kernel must reconcile by operation ID.
    Unknown,
}

/// The provider's account of a dispatched operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservedOutcome {
    /// The operation is known to have completed.
    Succeeded { result_ref: String },
    /// The operation is known to have failed.
    Failed { error_code: String },
    /// The provider has no record of the operation identity.
    NotFound,
    /// The observation was inconclusive.
    Unknown,
}

/// The kernel's plan after observing a `Dispatched` effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcilePlan {
    /// Apply the observed terminal outcome.
    Settle { observed: ObservedOutcome },
    /// The provider never saw the operation and the contract allows a safe
    /// redispatch of the same operation identity.
    Redispatch,
    /// Reconciliation was inconclusive or impossible: park in `Unknown`.
    Unknown,
}

/// Decides the reconciler's next step from the persisted contract and the
/// adapter's observation.
///
/// An observation of `NotFound` permits redispatch only when the contract is
/// safely redispatchable; every inconclusive case yields `Unknown` — an
/// `Unknown` row is never reopened by the reconciler.
pub fn reconcile_plan(row: &EffectRow, observed: ObservedOutcome) -> ReconcilePlan {
    if row.state != EffectState::Dispatched {
        return ReconcilePlan::Unknown;
    }
    match observed {
        ObservedOutcome::Succeeded { .. } | ObservedOutcome::Failed { .. } => {
            ReconcilePlan::Settle { observed }
        }
        ObservedOutcome::NotFound => {
            let contract = persisted_contract(row);
            if contract.is_safely_redispatchable() {
                ReconcilePlan::Redispatch
            } else {
                ReconcilePlan::Unknown
            }
        }
        ObservedOutcome::Unknown => ReconcilePlan::Unknown,
    }
}

/// Applies a [`ReconcilePlan`] inside `txn`, staging the transition events and
/// a terminal `EffectReconciled` audit record.
///
/// `Redispatch` re-stamps the row `Dispatched` with a fresh fencing token so a
/// later claim sees a claimable effect — the operation identity is unchanged.
pub async fn apply_observed(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    plan: ReconcilePlan,
) -> errors::Result<EffectRow> {
    let persisted = txn.effects().get(effect_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::NotFound,
            RetryClass::Never,
            format!("effect {effect_id} does not exist"),
        )
    })?;
    // Observation applies only to a row awaiting a lost Dispatched response;
    // terminal and already-Unknown rows are settled as-is.
    if persisted.state != EffectState::Dispatched {
        return Ok(persisted);
    }
    match plan {
        ReconcilePlan::Settle {
            observed: ObservedOutcome::Succeeded { result_ref },
        } => {
            transition(
                txn,
                env,
                effect_id,
                None,
                Transition {
                    to: EffectState::Acknowledged,
                    event_type: "EffectAcknowledged",
                    patch: EffectPatch {
                        state: Some(EffectState::Acknowledged),
                        result_ref: Some(result_ref),
                        ..EffectPatch::default()
                    },
                },
            )
            .await?;
            let row = transition(
                txn,
                env,
                effect_id,
                None,
                Transition {
                    to: EffectState::Committed,
                    event_type: "EffectCommitted",
                    patch: EffectPatch {
                        state: Some(EffectState::Committed),
                        ..EffectPatch::default()
                    },
                },
            )
            .await?;
            stage_reconciled(txn, env, effect_id, &row).await?;
            Ok(row)
        }
        ReconcilePlan::Settle {
            observed: ObservedOutcome::Failed { error_code },
        } => {
            let row = transition(
                txn,
                env,
                effect_id,
                None,
                Transition {
                    to: EffectState::Failed,
                    event_type: "EffectFailed",
                    patch: EffectPatch {
                        state: Some(EffectState::Failed),
                        error_code: Some(error_code),
                        ..EffectPatch::default()
                    },
                },
            )
            .await?;
            stage_reconciled(txn, env, effect_id, &row).await?;
            Ok(row)
        }
        ReconcilePlan::Redispatch => {
            transition(
                txn,
                env,
                effect_id,
                None,
                Transition {
                    to: EffectState::Dispatched,
                    event_type: "EffectDispatched",
                    patch: EffectPatch {
                        state: Some(EffectState::Dispatched),
                        ..EffectPatch::default()
                    },
                },
            )
            .await
        }
        ReconcilePlan::Settle {
            observed: ObservedOutcome::NotFound | ObservedOutcome::Unknown,
        }
        | ReconcilePlan::Unknown => {
            transition(
                txn,
                env,
                effect_id,
                None,
                Transition {
                    to: EffectState::Unknown,
                    event_type: "EffectUnknown",
                    patch: EffectPatch {
                        state: Some(EffectState::Unknown),
                        ..EffectPatch::default()
                    },
                },
            )
            .await
        }
    }
}

/// Resolution actions accepted by the `ResolveUnknownEffect` command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Operator asserts the operation succeeded; settle to `Committed`.
    MarkSucceeded,
    /// Operator asserts the operation failed; settle to `Failed`.
    MarkFailed,
    /// Re-`Prepared` the same operation identity for a fresh claim, accepting
    /// the risk the provider already applied it.
    RetryAcceptingDuplicateRisk,
}

impl Resolution {
    /// Parses the wire `action` token of `ResolveUnknownEffect`.
    pub fn parse(action: &str) -> errors::Result<Self> {
        match action {
            "mark_succeeded" => Ok(Self::MarkSucceeded),
            "mark_failed" => Ok(Self::MarkFailed),
            "retry_accepting_duplicate_risk" => Ok(Self::RetryAcceptingDuplicateRisk),
            _ => Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                format!("unknown ResolveUnknownEffect action {action:?}"),
            )),
        }
    }
}

/// Applies an operator resolution to an `Unknown` effect inside `txn`.
///
/// The caller must have already verified the row's `expected_effect_state`
/// and the required `effect.resolve_unknown` capability/approval. A settled
/// effect stages `EffectReconciled`; a retry re-arms the row as `Prepared`
/// without minting a new operation identity.
pub async fn apply_resolution(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    resolution: Resolution,
    result_ref: Option<String>,
    reason: &str,
) -> errors::Result<EffectRow> {
    let (to, event_type, patch) = match resolution {
        Resolution::MarkSucceeded => (
            EffectState::Committed,
            "EffectCommitted",
            EffectPatch {
                state: Some(EffectState::Committed),
                result_ref,
                ..EffectPatch::default()
            },
        ),
        Resolution::MarkFailed => (
            EffectState::Failed,
            "EffectFailed",
            EffectPatch {
                state: Some(EffectState::Failed),
                error_code: Some(if reason.is_empty() {
                    "resolved_failed".to_owned()
                } else {
                    reason.to_owned()
                }),
                ..EffectPatch::default()
            },
        ),
        Resolution::RetryAcceptingDuplicateRisk => (
            EffectState::Prepared,
            "EffectPrepared",
            EffectPatch {
                state: Some(EffectState::Prepared),
                ..EffectPatch::default()
            },
        ),
    };
    let row = transition(
        txn,
        env,
        effect_id,
        None,
        Transition {
            to,
            event_type,
            patch,
        },
    )
    .await?;
    stage_reconciled(txn, env, effect_id, &row).await?;
    Ok(row)
}

/// Reconstructs the persisted contract for safety checks.
fn persisted_contract(row: &EffectRow) -> EffectContract {
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
}

async fn stage_reconciled(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    row: &EffectRow,
) -> errors::Result<()> {
    stage_effect_event(
        txn,
        EventId::new(env.ids),
        "EffectReconciled",
        effect_id,
        effect_record(row).encode_to_vec(),
        env.correlation_id.clone(),
        None,
    )
    .await?;
    Ok(())
}
