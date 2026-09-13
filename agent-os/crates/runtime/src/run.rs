//! Revisioned run transitions over the normative state table.

use domain::ids::RunId;
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{RunCas, RunPatch, RunRow};

use crate::state::{REASON_CANCELLED, REASON_COMPLETED, REASON_FAILED, allows};

/// Loads the run, enforces the transition table and expected revision,
/// applies the CAS update, and increments the revision exactly once.
///
/// Rejections are `Conflict` with retry class `Never` and leave the row
/// untouched: the table is checked first, then `expect_revision`, then the
/// compare-and-set. Terminal targets persist a stable reason: `reason` when
/// the caller supplies one, otherwise the canonical `completed`/`failed`/
/// `cancelled` token. Non-terminal targets never write `terminal_reason`.
///
/// The mutation timestamp is accepted for signature stability with the
/// change-time stamping path; the CAS patch surface does not carry
/// `updated_at_ms` yet.
pub async fn transition(
    txn: &mut dyn KernelTxn,
    run_id: RunId,
    expect_revision: u64,
    to: RunState,
    reason: Option<String>,
    _now_ms: i64,
) -> errors::Result<RunRow> {
    let current = txn.runs().get(run_id).await?.ok_or_else(|| {
        KernelError::new(ErrorCode::NotFound, RetryClass::Never, "run does not exist")
    })?;
    if !allows(current.state, to) {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "run state transition is not permitted",
        ));
    }
    if current.run_revision != expect_revision {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "run revision does not match",
        ));
    }

    let updated = txn
        .runs()
        .cas_update(
            run_id,
            RunCas {
                run_revision: expect_revision,
                state: Some(current.state),
                cancellation_epoch: None,
            },
            RunPatch {
                state: Some(to),
                terminal_reason: terminal_reason(to, reason),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await?;
    if !updated {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "run transition lost a revision race",
        ));
    }

    txn.runs().get(run_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "run vanished after transition",
        )
    })
}

fn terminal_reason(to: RunState, reason: Option<String>) -> Option<String> {
    match to {
        RunState::Completed => Some(reason.unwrap_or_else(|| REASON_COMPLETED.to_owned())),
        RunState::Failed => Some(reason.unwrap_or_else(|| REASON_FAILED.to_owned())),
        RunState::Cancelled => Some(reason.unwrap_or_else(|| REASON_CANCELLED.to_owned())),
        _ => None,
    }
}
