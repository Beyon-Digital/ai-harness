//! Durable loop turns (LOOP-002): a turn is the fence tuple the loop's
//! `LoopInput`/`LoopDecision` exchange carries — `run_id`, the
//! post-issue `run_revision`, `loop_epoch`, `step_sequence`,
//! `input_event_cursor`, and `turn_id`.
//!
//! Issuing a turn atomically stamps the run's next step and revision
//! before the `issued` row exists, so a crash between "issued to the
//! loop process" and "decision received" leaves a durable record to
//! reconcile (recovery marks orphaned issued turns stale).
#![forbid(unsafe_code)]

use domain::ids::{RunId, TurnId};
use domain::provider::IdProvider;
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::LoopTurnRow;
use kernel_store::models::{LoopTurnPatch, NewLoopTurn, RunCas, RunPatch};
use kernel_store::txn::KernelTxn;

/// `loop_turns.state` literals.
pub mod turn_state {
    /// Sent to the loop, awaiting a decision.
    pub const ISSUED: &str = "issued";
    /// A decision was durably accepted for the turn.
    pub const ACCEPTED: &str = "accepted";
    /// The fence moved on without this turn (restart / re-issue).
    pub const STALE: &str = "stale";
}

fn turn_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Issue the next turn for `run`: CAS the run to `Running` with
/// `step_sequence + 1` and a bumped revision, then insert the `issued`
/// turn carrying the post-bump fence. The whole operation is one
/// transaction's work — callers commit.
///
/// Fails `FailedPrecondition` on terminal runs and `Conflict` when the
/// expected revision moved (a concurrent issuer loses).
pub async fn issue_turn(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    run: RunId,
    now_ms: i64,
) -> errors::Result<LoopTurnRow> {
    let row = txn
        .runs()
        .get(run)
        .await?
        .ok_or_else(|| turn_error(ErrorCode::NotFound, "run not found"))?;
    if row.state.is_terminal() {
        return Err(turn_error(
            ErrorCode::FailedPrecondition,
            format!("run is terminal ({:?}) — no further turns", row.state),
        ));
    }
    if matches!(
        row.state,
        RunState::Cancelling | RunState::Suspended | RunState::WaitingHuman
    ) {
        return Err(turn_error(
            ErrorCode::FailedPrecondition,
            format!("run state {:?} is not turn-eligible", row.state),
        ));
    }
    let next_step = row.step_sequence + 1;
    let next_revision = row.run_revision + 1;
    let moved = txn
        .runs()
        .cas_update(
            run,
            RunCas {
                run_revision: row.run_revision,
                state: None,
                cancellation_epoch: Some(row.cancellation_epoch),
            },
            RunPatch {
                state: Some(RunState::Running),
                step_sequence: Some(next_step),
                bump_revision: true,
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(turn_error(
            ErrorCode::Conflict,
            "run moved during turn issue",
        ));
    }
    let turn_id = TurnId::new(ids);
    txn.loop_turns()
        .insert_turn(NewLoopTurn {
            turn_id,
            run_id: run,
            run_revision: next_revision,
            loop_epoch: row.loop_epoch,
            step_sequence: next_step,
            input_event_cursor: row.input_event_cursor.clone(),
            state: turn_state::ISSUED.to_owned(),
            issued_at_ms: now_ms,
        })
        .await?;
    txn.loop_turns()
        .get_turn(turn_id)
        .await?
        .ok_or_else(|| turn_error(ErrorCode::Internal, "issued turn not visible"))
}

/// Rebind / restart path: increment `loop_epoch` and mark every
/// outstanding `issued` turn `stale`. Stale decisions for the old epoch
/// then fail validation without side effects.
///
/// `issued` turn ids are caller-supplied (the kernel store exposes no
/// list op by run/state for turns); the worker tracks them.
pub async fn bump_loop_epoch(
    txn: &mut dyn KernelTxn,
    run: RunId,
    expected_revision: u64,
    outstanding_issued: &[TurnId],
) -> errors::Result<u64> {
    let row = txn
        .runs()
        .get(run)
        .await?
        .ok_or_else(|| turn_error(ErrorCode::NotFound, "run not found"))?;
    for turn_id in outstanding_issued {
        let _ = txn
            .loop_turns()
            .cas_turn(
                *turn_id,
                turn_state::ISSUED,
                LoopTurnPatch {
                    state: Some(turn_state::STALE.to_owned()),
                },
            )
            .await?;
    }
    let moved = txn
        .runs()
        .cas_update(
            run,
            RunCas {
                run_revision: expected_revision,
                state: None,
                cancellation_epoch: Some(row.cancellation_epoch),
            },
            RunPatch {
                loop_epoch: Some(row.loop_epoch + 1),
                bump_revision: true,
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(turn_error(
            ErrorCode::Conflict,
            "run moved during loop epoch bump",
        ));
    }
    Ok(row.loop_epoch + 1)
}
