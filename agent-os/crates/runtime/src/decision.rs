//! Loop decision validation + durable acceptance (LOOP-002).
//!
//! A `LoopDecision` is accepted only when its fence tuple matches the
//! issued turn exactly; a replayed `decision_id` returns the recorded
//! outcome without re-running side effects, and a stale fence is
//! rejected before any state moves.
#![forbid(unsafe_code)]

use domain::generated::contract::{LoopDecision, loop_decision};
use domain::ids::{DecisionId, EffectId, EventId, RunId, TurnId};
use domain::provider::IdProvider;
use domain::run::RunState;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::models::{DecisionRow, LoopTurnPatch, NewDecision, RunCas, RunPatch};
use kernel_store::txn::KernelTxn;
use sha2::{Digest, Sha256};

use crate::loop_turn::turn_state;
use crate::stage_catalogued;

fn dec_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// What an accepted decision instructs the coordinator to prepare —
/// the durable transition already happened; these carry the work items
/// (effect claim bytes, child spec, wait) the worker must act on next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionInstruction {
    /// Run completed; `output_ref` is the artifact URI.
    Complete {
        /// Output reference recorded on the run.
        output_ref: String,
    },
    /// Run failed with a loop-reported reason.
    Fail {
        /// Terminal reason recorded on the run.
        reason_code: String,
    },
    /// Run waits on a durable timer.
    Wait {
        /// Human-readable wait reason.
        reason: String,
        /// Timer identifier the scheduler owns.
        timer_id: String,
    },
    /// Run stays `Running` waiting on a child run to be created.
    SpawnAgent {
        /// Opaque `CreateTaskRun` contract bytes for the child.
        child_request: Vec<u8>,
    },
    /// Run stays `Running` waiting on a fenced effect claim.
    InvokeEffect {
        /// Port operation the effect invokes.
        operation: String,
        /// Operation payload.
        payload: Vec<u8>,
        /// `EffectClaim` contract bytes to prepare.
        effect_claim: Vec<u8>,
    },
    /// Run waits on a human approval draft.
    RequestApproval {
        /// `ApprovalRequest` contract bytes to prepare.
        approval_draft: Vec<u8>,
    },
}

/// Result of [`accept_decision`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// First-time acceptance: the decision row is durable.
    Accepted {
        /// Recorded decision row.
        row: DecisionRow,
        /// Work the coordinator must drive next.
        instruction: DecisionInstruction,
    },
    /// `decision_id` was already accepted — the prior outcome replays
    /// with no new side effects.
    Replayed {
        /// Previously recorded decision row.
        prior: DecisionRow,
    },
}

fn decision_type_and_instruction(
    decision: &LoopDecision,
) -> errors::Result<(&'static str, DecisionInstruction, RunState)> {
    let Some(kind) = &decision.decision else {
        return Err(dec_error(
            ErrorCode::InvalidArgument,
            "decision body missing",
        ));
    };
    Ok(match kind {
        loop_decision::Decision::Complete(c) => (
            "Complete",
            DecisionInstruction::Complete {
                output_ref: c.output_ref.clone(),
            },
            RunState::Completed,
        ),
        loop_decision::Decision::Fail(f) => (
            "Fail",
            DecisionInstruction::Fail {
                reason_code: f.reason_code.clone(),
            },
            RunState::Failed,
        ),
        loop_decision::Decision::Wait(w) => (
            "Wait",
            DecisionInstruction::Wait {
                reason: w.reason.clone(),
                timer_id: w.timer_id.clone(),
            },
            RunState::WaitingTool,
        ),
        loop_decision::Decision::SpawnAgent(s) => (
            "SpawnAgent",
            DecisionInstruction::SpawnAgent {
                child_request: s.child_request.clone(),
            },
            RunState::WaitingChild,
        ),
        loop_decision::Decision::InvokeEffect(e) => (
            "InvokeEffect",
            DecisionInstruction::InvokeEffect {
                operation: e.operation.clone(),
                payload: e.payload.clone(),
                effect_claim: e.effect_claim.clone(),
            },
            // The run parks on the prepared effect; the worker dispatches
            // it, then settles the wait (specs/command-catalog.md).
            RunState::WaitingTool,
        ),
        loop_decision::Decision::RequestApproval(a) => (
            "RequestApproval",
            DecisionInstruction::RequestApproval {
                approval_draft: a.approval_draft.clone(),
            },
            RunState::WaitingHuman,
        ),
    })
}

/// Accept (or reject) a `LoopDecision` for `run`. Atomically: validate
/// the fence tuple against the issued turn, insert the decision row,
/// mark the turn accepted, and move the run to the state the decision
/// implies. Replay: an already-recorded `decision_id` returns
/// [`AcceptOutcome::Replayed`] and touches nothing.
///
/// Rejections: unknown turn → `NotFound`; fence mismatch (revision,
/// epoch, step, cursor, turn) → `Conflict` and the issued turn is
/// marked `stale`; a losing run CAS → `Conflict`.
pub async fn accept_decision(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    clock: &dyn Clock,
    run: RunId,
    decision: &LoopDecision,
    decision_bytes: &[u8],
    now_ms: i64,
) -> errors::Result<AcceptOutcome> {
    let decision_id = decision
        .decision_id
        .parse::<DecisionId>()
        .map_err(|_| dec_error(ErrorCode::InvalidArgument, "decision_id is not an id"))?;
    if let Some(prior) = txn.loop_turns().get_decision(run, decision_id).await? {
        return Ok(AcceptOutcome::Replayed { prior });
    }
    let turn_id = decision
        .turn_id
        .parse::<TurnId>()
        .map_err(|_| dec_error(ErrorCode::InvalidArgument, "turn_id is not an id"))?;
    let turn = txn
        .loop_turns()
        .get_turn(turn_id)
        .await?
        .ok_or_else(|| dec_error(ErrorCode::NotFound, "turn not found for decision"))?;
    if turn.run_id != run {
        return Err(dec_error(
            ErrorCode::FailedPrecondition,
            "decision turn belongs to a different run",
        ));
    }
    if turn.state != turn_state::ISSUED {
        return Err(dec_error(
            ErrorCode::FailedPrecondition,
            format!(
                "turn is {state} — not accepting decisions",
                state = turn.state
            ),
        ));
    }
    let cursor_ok = decision.input_event_cursor == turn.input_event_cursor.to_string();
    let fence_ok = decision.run_revision == turn.run_revision
        && decision.loop_epoch == turn.loop_epoch
        && decision.step_sequence == turn.step_sequence
        && cursor_ok;
    if !fence_ok {
        // Stale decision: ledger the turn stale, apply no other effects.
        let _ = txn
            .loop_turns()
            .cas_turn(
                turn_id,
                turn_state::ISSUED,
                LoopTurnPatch {
                    state: Some(turn_state::STALE.to_owned()),
                },
            )
            .await?;
        return Err(dec_error(
            ErrorCode::Conflict,
            "stale decision: fence tuple does not match the issued turn",
        ));
    }
    let (decision_type, instruction, next_state) = decision_type_and_instruction(decision)?;
    txn.loop_turns()
        .insert_decision(NewDecision {
            decision_id,
            run_id: run,
            turn_id,
            decision_type: decision_type.to_owned(),
            decision_digest: format!("sha256:{:x}", Sha256::digest(decision_bytes)),
            decision_bytes: decision_bytes.to_vec(),
            run_revision: turn.run_revision,
            loop_epoch: turn.loop_epoch,
            step_sequence: turn.step_sequence,
            input_event_cursor: turn.input_event_cursor.clone(),
            accepted_at_ms: now_ms,
        })
        .await?;
    let accepted = txn
        .loop_turns()
        .cas_turn(
            turn_id,
            turn_state::ISSUED,
            LoopTurnPatch {
                state: Some(turn_state::ACCEPTED.to_owned()),
            },
        )
        .await?;
    if !accepted {
        return Err(dec_error(ErrorCode::Conflict, "turn accepted concurrently"));
    }
    let moved = txn
        .runs()
        .cas_update(
            run,
            RunCas {
                run_revision: turn.run_revision,
                state: None,
                cancellation_epoch: None,
            },
            RunPatch {
                state: Some(next_state),
                output_ref: match &instruction {
                    DecisionInstruction::Complete { output_ref } => Some(output_ref.clone()),
                    _ => None,
                },
                terminal_reason: match &instruction {
                    DecisionInstruction::Fail { reason_code } => Some(reason_code.clone()),
                    _ => None,
                },
                bump_revision: true,
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(dec_error(
            ErrorCode::Conflict,
            "run moved during decision accept",
        ));
    }
    // An accepted `InvokeEffect` prepares the durable effect record in the
    // same transaction as the decision (specs/command-catalog.md:
    // "Acceptance and resulting mutation/effect/child preparation are
    // atomic"; emits `EffectPrepared` + `RunWaitingTool`).
    if let DecisionInstruction::InvokeEffect {
        operation, payload, ..
    } = &instruction
    {
        let effect_env = effects::EffectEnv {
            ids,
            clock,
            correlation_id: None,
            causation_id: None,
        };
        prepare_effect_in_txn(
            txn,
            &effect_env,
            run,
            decision,
            turn.step_sequence,
            operation,
            payload,
        )
        .await?;
    }
    let row = txn
        .loop_turns()
        .get_decision(run, decision_id)
        .await?
        .ok_or_else(|| dec_error(ErrorCode::Internal, "decision row not visible"))?;

    // SubmitLoopDecision emits LoopDecisionAccepted plus the state event the
    // transition implies (specs/command-catalog.md, event-catalog.md).
    stage_catalogued(
        txn,
        EventId::new(ids),
        "LoopDecisionAccepted",
        StreamKey::run(run),
        decision_bytes.to_vec(),
        None,
        None,
    )
    .await?;
    let state_event = match next_state {
        RunState::Completed => Some("RunCompleted"),
        RunState::Failed => Some("RunFailed"),
        RunState::WaitingTool => Some("RunWaitingTool"),
        RunState::WaitingChild => Some("RunWaitingChild"),
        RunState::WaitingHuman => Some("RunWaitingHuman"),
        _ => None,
    };
    if let Some(state_event) = state_event {
        let payload = decision_bytes.to_vec();
        for event_type in [state_event, "RunStateChanged"] {
            stage_catalogued(
                txn,
                EventId::new(ids),
                event_type,
                StreamKey::run(run),
                payload.clone(),
                None,
                None,
            )
            .await?;
        }
    }
    Ok(AcceptOutcome::Accepted { row, instruction })
}

/// Prepares the durable `Prepared` effect for an accepted `InvokeEffect`
/// inside the decision transaction.
///
/// The adapter binding comes from the run's frozen `resolved_bindings`
/// (`effect.execute` port); the effective contract is the conservative
/// policy resolution over kernel-known semantics plus the bound adapter's
/// trust/conformance state.
async fn prepare_effect_in_txn(
    txn: &mut dyn KernelTxn,
    env: &effects::EffectEnv<'_>,
    run: RunId,
    decision: &LoopDecision,
    step_sequence: u64,
    operation: &str,
    payload: &[u8],
) -> errors::Result<()> {
    let row = txn
        .runs()
        .get(run)
        .await?
        .ok_or_else(|| dec_error(ErrorCode::Internal, "run vanished mid-accept"))?;
    let environment_id = row.resolved_environment_id.ok_or_else(|| {
        dec_error(
            ErrorCode::FailedPrecondition,
            "InvokeEffect before environment binding",
        )
    })?;
    let binding = txn
        .environments()
        .get_bindings(environment_id)
        .await?
        .into_iter()
        .find(|b| b.port_id == "effect.execute")
        .ok_or_else(|| {
            dec_error(
                ErrorCode::FailedPrecondition,
                "frozen environment has no effect.execute binding",
            )
        })?;
    let registration = txn
        .adapters()
        .get_registration(
            binding.adapter_id,
            &binding.adapter_version,
            &binding.adapter_digest,
        )
        .await?
        .ok_or_else(|| {
            dec_error(
                ErrorCode::FailedPrecondition,
                "bound effect adapter is not registered",
            )
        })?;
    let contract = effects::resolve(&effects::ResolveInputs {
        kernel: Some(effects::kernel_declared(operation)),
        claim: None,
        trust: registration.trust_state,
        conformance: registration.conformance_state,
        policy: None,
    });
    let decision_id = decision
        .decision_id
        .parse::<DecisionId>()
        .map_err(|_| dec_error(ErrorCode::InvalidArgument, "decision_id is not an id"))?;
    effects::prepare_effect(
        txn,
        env,
        effects::PrepareRequest {
            effect_id: EffectId::new(env.ids),
            run_id: run,
            step_sequence,
            decision_id,
            operation: operation.to_owned(),
            request_payload: payload.to_vec(),
            adapter: effects::AdapterBinding {
                adapter_id: binding.adapter_id,
                adapter_version: binding.adapter_version,
                adapter_digest: binding.adapter_digest,
            },
            contract,
        },
    )
    .await?;
    Ok(())
}
