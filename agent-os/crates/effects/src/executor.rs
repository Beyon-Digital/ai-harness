//! Executor leases and fencing: at most one current executor may advance an
//! effect at a time.
//!
//! [`claim`] is the single atomic hand-off: it requires `Prepared` or an
//! expired `Claimed` lease, writes the executor identity, stamps the daemon
//! fencing epoch, and monotonically increases `executor_fencing_token` inside
//! the same `UPDATE`. Every later transition re-verifies executor identity,
//! fencing token, daemon epoch, and lease freshness against the persisted row
//! before its CAS, so a stale executor's result is diagnostic only and can
//! never commit.

use domain::effect::EffectState;
use domain::ids::EffectId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{EffectPatch, EffectRow};
use prost::Message;

use crate::coordinator::effect_record;
use crate::env::EffectEnv;
use crate::record::can_transition;
use crate::stage::stage_effect_event;
use domain::ids::EventId;

/// Lease duration granted to an effect claim (`effects.lease_ms` in
/// `specs/limits.yaml`).
pub const EFFECT_LEASE_MS: i64 = 30_000;

/// Outcome of an executor claim attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The executor holds the claim; the new fencing token is authoritative.
    Claimed {
        /// The fencing token stamped on the claim.
        fencing_token: u64,
        /// The row as committed by the claim.
        row: EffectRow,
    },
    /// Another executor holds a live claim, or the effect already left the
    /// claimable states.
    Busy(EffectRow),
}

/// Executor credentials carried on every post-claim transition.
#[derive(Clone, Debug)]
pub struct ExecutorRef<'a> {
    /// Claiming executor identity.
    pub executor_id: &'a str,
    /// Fencing token returned by [`claim`].
    pub fencing_token: u64,
}

/// Atomically claims `effect_id` for `executor_id` inside `txn`.
///
/// The predicate is `state = Prepared OR (state = Claimed AND lease expired)`;
/// on success the fencing token increments and `EffectClaimed` is staged.
/// The lease is always [`EFFECT_LEASE_MS`]: lease duration is kernel policy,
/// not an executor choice.
pub async fn claim(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor_id: &str,
) -> errors::Result<ClaimOutcome> {
    let now_ms = env.clock.now_unix_ms();
    let daemon_epoch = txn.context().daemon_epoch;
    let token = txn
        .effects()
        .claim(
            effect_id,
            executor_id,
            daemon_epoch,
            now_ms + EFFECT_LEASE_MS,
            now_ms,
        )
        .await?;
    let row = txn
        .effects()
        .get(effect_id)
        .await?
        .ok_or_else(|| not_found(effect_id))?;
    match token {
        Some(fencing_token) => {
            stage_effect_event(
                txn,
                EventId::new(env.ids),
                "EffectClaimed",
                effect_id,
                effect_record(&row).encode_to_vec(),
                env.correlation_id.clone(),
                None,
            )
            .await?;
            Ok(ClaimOutcome::Claimed { fencing_token, row })
        }
        None => Ok(ClaimOutcome::Busy(row)),
    }
}

/// Persists `Dispatched` before the adapter call (the dispatcher commits this
/// transaction before any external I/O).
pub async fn mark_dispatched(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: &ExecutorRef<'_>,
    provider_operation_ref: Option<String>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        Some(executor),
        Transition {
            to: EffectState::Dispatched,
            event_type: "EffectDispatched",
            patch: EffectPatch {
                state: Some(EffectState::Dispatched),
                provider_operation_ref,
                ..EffectPatch::default()
            },
        },
    )
    .await
}

/// `Dispatched -> Acknowledged`: the adapter reported a completed operation.
pub async fn acknowledge(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: &ExecutorRef<'_>,
    result_ref: Option<String>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        Some(executor),
        Transition {
            to: EffectState::Acknowledged,
            event_type: "EffectAcknowledged",
            patch: EffectPatch {
                state: Some(EffectState::Acknowledged),
                result_ref,
                ..EffectPatch::default()
            },
        },
    )
    .await
}

/// `Acknowledged -> Committed`: the kernel commits the reported result.
pub async fn commit(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: &ExecutorRef<'_>,
    result_ref: Option<String>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        Some(executor),
        Transition {
            to: EffectState::Committed,
            event_type: "EffectCommitted",
            patch: EffectPatch {
                state: Some(EffectState::Committed),
                result_ref,
                ..EffectPatch::default()
            },
        },
    )
    .await
}

/// `Dispatched|Acknowledged -> Failed`: the executor reports a terminal
/// adapter-side failure.
pub async fn fail(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: &ExecutorRef<'_>,
    error_code: impl Into<String>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        Some(executor),
        Transition {
            to: EffectState::Failed,
            event_type: "EffectFailed",
            patch: EffectPatch {
                state: Some(EffectState::Failed),
                error_code: Some(error_code.into()),
                ..EffectPatch::default()
            },
        },
    )
    .await
}

/// `Dispatched -> Unknown`: the response was lost and reconciliation is
/// impossible or inconclusive. The kernel never auto-retries `Unknown`.
pub async fn mark_unknown(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: Option<&ExecutorRef<'_>>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        executor,
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

/// `Prepared|Claimed|Dispatched -> Cancelled`.
pub async fn cancel(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: Option<&ExecutorRef<'_>>,
) -> errors::Result<EffectRow> {
    transition(
        txn,
        env,
        effect_id,
        executor,
        Transition {
            to: EffectState::Cancelled,
            event_type: "EffectCancelled",
            patch: EffectPatch {
                state: Some(EffectState::Cancelled),
                ..EffectPatch::default()
            },
        },
    )
    .await
}

/// One requested transition: target state, staged event, and field patch.
pub(crate) struct Transition {
    pub to: EffectState,
    pub event_type: &'static str,
    pub patch: EffectPatch,
}

/// Applies a single legal transition with executor-fencing verification.
///
/// The persisted row's state must be a legal `from` per the transition table,
/// and — when `executor` is supplied — the executor identity, fencing token,
/// daemon epoch, and lease freshness are verified against the row before the
/// CAS.
pub(crate) async fn transition(
    txn: &mut dyn KernelTxn,
    env: &EffectEnv<'_>,
    effect_id: EffectId,
    executor: Option<&ExecutorRef<'_>>,
    transition: Transition,
) -> errors::Result<EffectRow> {
    let row = txn
        .effects()
        .get(effect_id)
        .await?
        .ok_or_else(|| not_found(effect_id))?;
    if !can_transition(row.state, transition.to) {
        return Err(illegal_transition(row.state, transition.to));
    }
    let expect_token = match executor {
        Some(executor) => Some(verify_executor(
            txn,
            &row,
            executor,
            env.clock.now_unix_ms(),
        )?),
        None => row.executor_fencing_token,
    };
    let applied = txn
        .effects()
        .cas_transition(effect_id, row.state, expect_token, transition.patch)
        .await?;
    if !applied {
        return Err(stale_executor());
    }
    let row = txn
        .effects()
        .get(effect_id)
        .await?
        .ok_or_else(|| not_found(effect_id))?;
    stage_effect_event(
        txn,
        EventId::new(env.ids),
        transition.event_type,
        effect_id,
        effect_record(&row).encode_to_vec(),
        env.correlation_id.clone(),
        None,
    )
    .await?;
    Ok(row)
}

/// Verifies the executor still holds the effect: identity, fencing token,
/// daemon epoch, and an unexpired lease.
fn verify_executor(
    txn: &mut dyn KernelTxn,
    row: &EffectRow,
    executor: &ExecutorRef<'_>,
    now_ms: i64,
) -> errors::Result<u64> {
    if row.executor_id.as_deref() != Some(executor.executor_id) {
        return Err(stale_executor());
    }
    if row.executor_fencing_token != Some(executor.fencing_token) {
        return Err(stale_executor());
    }
    let daemon_epoch = txn.context().daemon_epoch;
    if row.daemon_fencing_epoch != Some(daemon_epoch) {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "executor daemon fencing epoch is stale",
        ));
    }
    if row.lease_expires_ms.is_some_and(|lease| lease <= now_ms) {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "executor lease has expired",
        ));
    }
    Ok(executor.fencing_token)
}

fn not_found(effect_id: EffectId) -> KernelError {
    KernelError::new(
        ErrorCode::NotFound,
        RetryClass::Never,
        format!("effect {effect_id} does not exist"),
    )
}

fn stale_executor() -> KernelError {
    KernelError::new(
        ErrorCode::Conflict,
        RetryClass::Never,
        "effect claim is held by a different executor or fencing token",
    )
}

fn illegal_transition(from: EffectState, to: EffectState) -> KernelError {
    KernelError::new(
        ErrorCode::FailedPrecondition,
        RetryClass::Never,
        format!("illegal effect transition {from:?} -> {to:?}"),
    )
}
