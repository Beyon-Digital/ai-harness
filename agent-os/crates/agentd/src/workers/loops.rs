//! Agent-loop worker (LOOP-002): drives one durable turn against a live
//! loop-adapter session.
//!
//! `drive_turn` is one iteration: durably issue the turn (kernel txn),
//! build the `LoopInput` carrying the turn's fence tuple, dispatch the
//! `agent_loop.next` port call to the frozen loop bundle's process, and
//! atomically accept the returned `LoopDecision` in a second kernel
//! txn. A crash between issue and accept leaves an `issued` turn for
//! recovery to mark stale; a replayed decision id replays its recorded
//! outcome with no duplicate effect/child/timer preparation.
//!
//! The composition root wires this worker in a later task, so its items
//! are not yet reachable from `main`.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use adapter_protocol::session::{self, CallError, SessionPhase};
use domain::generated::contract::{LoopDecision, LoopInput, PortCallRequest};
use domain::ids::RunId;
use domain::provider::IdProvider;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::LoopTurnRow;
use kernel_store::{KernelStore, TxContext};
use process_supervisor::spawn::Child;
use prost::Message;

use runtime::decision::{self, AcceptOutcome};
use runtime::loop_turn;

#[allow(dead_code)]
fn worker_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Port + operation the loop bundle implements.
#[allow(dead_code)]
pub const LOOP_PORT_ID: &str = "agent_loop";
/// `agent_loop.next` — the single operation a loop adapter serves.
#[allow(dead_code)]
pub const LOOP_NEXT_OPERATION: &str = "next";

/// One `issue -> call -> accept` cycle for `run` over the loop
/// process's IPC channel.
///
/// `state`/`events` are the opaque run-state snapshot and new-events
/// batch the frozen loop bundle consumes; the kernel treats them as
/// payload bytes.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub async fn drive_turn<S>(
    store: &S,
    ids: &dyn IdProvider,
    clock: &dyn Clock,
    ctx: &TxContext,
    child: &mut Child,
    phase: &mut SessionPhase,
    run: RunId,
    state: Vec<u8>,
    events: Vec<u8>,
    call_deadline: Duration,
    now_ms: i64,
) -> errors::Result<AcceptOutcome>
where
    S: KernelStore + ?Sized,
{
    // 1. Durable issue: the fence tuple is fixed inside the txn.
    let turn = {
        let mut txn = store.begin_write(ctx.clone()).await?;
        let turn = loop_turn::issue_turn(&mut *txn, ids, run, now_ms).await?;
        txn.commit().await?;
        turn
    };

    // 2. Send LoopInput to the frozen loop bundle.
    let input = LoopInput {
        run_id: turn.run_id.to_string(),
        run_revision: turn.run_revision,
        loop_epoch: turn.loop_epoch,
        step_sequence: turn.step_sequence,
        input_event_cursor: turn.input_event_cursor.to_string(),
        turn_id: turn.turn_id.to_string(),
        state,
        events,
    };
    let request = PortCallRequest {
        call_id: format!("loop-{}", turn.turn_id),
        port_id: LOOP_PORT_ID.to_owned(),
        operation: LOOP_NEXT_OPERATION.to_owned(),
        context: None,
        payload: input.encode_to_vec(),
    };
    let response =
        match session::dispatch_call(child.ipc(), phase, request, Instant::now() + call_deadline) {
            Ok(response) => response,
            Err(CallError::Cancelled(id)) => {
                return Err(worker_error(
                    ErrorCode::Unavailable,
                    format!("loop call {id} cancelled"),
                ));
            }
            Err(CallError::Kernel(e)) => return Err(e),
        };
    if !response.error_code.is_empty() {
        return Err(worker_error(
            ErrorCode::FailedPrecondition,
            format!("loop adapter rejected the call: {}", response.error_code),
        ));
    }
    let decision = LoopDecision::decode(response.payload.as_slice())
        .map_err(|_| worker_error(ErrorCode::InvalidArgument, "loop response did not decode"))?;

    // 3. Atomic accept: validate fence, record the decision, move the
    //    run, and produce the follow-up instruction.
    accept_with_new_txn(
        store,
        ids,
        clock,
        ctx,
        turn,
        &decision,
        &response.payload,
        now_ms,
    )
    .await
}

/// Separate txn for accept so the issue commits even if the process
/// dies mid-call (the `issued` turn is then reconciled as stale on
/// restart/rebind).
#[allow(clippy::too_many_arguments)]
async fn accept_with_new_txn<S>(
    store: &S,
    ids: &dyn IdProvider,
    clock: &dyn Clock,
    ctx: &TxContext,
    turn: LoopTurnRow,
    decision: &LoopDecision,
    decision_bytes: &[u8],
    now_ms: i64,
) -> errors::Result<AcceptOutcome>
where
    S: KernelStore + ?Sized,
{
    let mut txn = store.begin_write(ctx.clone()).await?;
    let outcome = decision::accept_decision(
        &mut *txn,
        ids,
        clock,
        turn.run_id,
        decision,
        decision_bytes,
        now_ms,
    )
    .await?;
    txn.commit().await?;
    Ok(outcome)
}
