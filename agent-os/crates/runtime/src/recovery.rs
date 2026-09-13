//! Startup reconstruction: classification and persistence of recovery
//! dispositions.
//!
//! [`reconstruct`] enumerates every non-terminal run under the acquired daemon
//! epoch, loads its effects, timers, and frozen environment bindings, and
//! assigns exactly one [`RecoveryDisposition`] per the normative recovery
//! matrix (`specs/recovery-table.md`, R6.2). Precedence is first-match:
//! blocked-unknown-effect, then reconciliation, then missing resource, then
//! the ordinary run-state row (D5). A `(run state, effect state)` pair no row
//! assigns fails closed with `Internal` before any disposition is written
//! (R6.3, D8).
//!
//! Changed dispositions are persisted with a revision-bumping patch and a
//! catalogued `RunRecoveryDispositionChanged` event; run state is never
//! touched and terminal runs are never enumerated (R6.4-R6.6, D7).

use std::sync::Arc;

use domain::effect::{EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::generated::contract;
use domain::ids::{CommandId, EventId, PrincipalId, RunId};
use domain::provider::SystemIdProvider;
use domain::resource::TimerState;
use domain::run::{RecoveryDisposition, RunState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::models::{
    EffectRow, ResolvedBindingRow, ResolvedEnvironmentRow, RunCas, RunPatch, RunRow, TimerRow,
};
use kernel_store::{KernelReadTxn, KernelStore, TxContext};
use prost::Message;
use run_graph::readiness::condition_met;

pub use domain::time::Clock;

use crate::stage_catalogued;
use crate::state::is_terminal;

/// Summary of one startup reconstruction pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Number of non-terminal runs examined.
    pub examined: usize,
    /// Classified disposition per examined run, in enumeration order.
    pub dispositions: Vec<(RunId, RecoveryDisposition)>,
}

/// Reconstructs startup state and persists every changed disposition.
///
/// The read pass classifies all active runs before the write pass begins, so
/// a fail-closed combination aborts startup with no partial disposition state.
/// `_clock` is carried for interface stability with the composition root, in
/// the same way [`crate::run::transition`] carries an unused mutation
/// timestamp; the disposition patch surface has no time column yet.
pub async fn reconstruct(
    store: &dyn KernelStore,
    _clock: Arc<dyn Clock>,
) -> errors::Result<RecoveryReport> {
    let fence = store.current_fence().await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "startup recovery requires an acquired daemon fence",
        )
    })?;

    let planned = {
        let mut read = store.begin_read().await?;
        let runs = read.runs().list_active().await?;
        let mut planned = Vec::with_capacity(runs.len());
        for run in runs {
            let effects = read.effects().list_by_run(run.run_id).await?;
            let timers = read.timers().list_by_run(run.run_id).await?;
            let disposition =
                classify(read.as_mut(), &run, &effects, &timers, fence.epoch.0).await?;
            planned.push((run, disposition));
        }
        planned
    };

    let examined = planned.len();
    let mut dispositions = Vec::with_capacity(examined);
    let mut changed = Vec::new();
    for (run, disposition) in &planned {
        dispositions.push((run.run_id, *disposition));
        if run.recovery != *disposition {
            changed.push((run.clone(), *disposition));
        }
    }

    if !changed.is_empty() {
        let context = TxContext {
            daemon_epoch: fence.epoch.0,
            principal_id: PrincipalId::new(&SystemIdProvider),
            command_id: CommandId::new(&SystemIdProvider),
            correlation_id: None,
        };
        let mut txn = store.begin_write(context).await?;
        for (run, disposition) in changed {
            let updated = txn
                .runs()
                .cas_update(
                    run.run_id,
                    RunCas {
                        run_revision: run.run_revision,
                        state: Some(run.state),
                        cancellation_epoch: None,
                    },
                    RunPatch {
                        recovery: Some(disposition),
                        bump_revision: true,
                        ..RunPatch::default()
                    },
                )
                .await?;
            if !updated {
                return Err(KernelError::new(
                    ErrorCode::Conflict,
                    RetryClass::Never,
                    "run changed during startup recovery",
                ));
            }
            let current = txn.runs().get(run.run_id).await?.ok_or_else(|| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "run vanished during startup recovery",
                )
            })?;
            stage_catalogued(
                txn.as_mut(),
                EventId::new(&SystemIdProvider),
                "RunRecoveryDispositionChanged",
                StreamKey::run(run.run_id),
                run_payload(&current).encode_to_vec(),
                None,
                None,
            )
            .await?;
        }
        txn.commit().await?;
    }

    Ok(RecoveryReport {
        examined,
        dispositions,
    })
}

/// Classifies one non-terminal run against the recovery matrix.
async fn classify(
    read: &mut dyn KernelReadTxn,
    run: &RunRow,
    effects: &[EffectRow],
    timers: &[TimerRow],
    daemon_epoch: u64,
) -> errors::Result<RecoveryDisposition> {
    let Some(class) = EffectClass::aggregate(effects) else {
        let offending = effects
            .iter()
            .find(|effect| effect.state == EffectState::Unspecified);
        return Err(unmapped(run, offending));
    };
    let Some(mut disposition) = matrix(run.state, class) else {
        let offending = effects.iter().find(|effect| is_in_flight(effect.state));
        return Err(unmapped(run, offending));
    };

    // Matrix D is applied last: an ordinary row matches only while the frozen
    // references its in-flight effects need resolve.
    if class.is_ordinary() {
        let (environment, bindings) = load_frozen_environment(read, run).await?;
        if missing_resource(run, effects, environment.as_ref(), &bindings) {
            disposition = RecoveryDisposition::BlockedMissingResource;
        }
    }

    // Matrix A distinguishes a `WaitingChild` whose declared conditions are
    // all satisfied (the parent must advance) from one whose children are
    // still running.
    if run.state == RunState::WaitingChild && class == EffectClass::None {
        disposition = if waiting_child_advanced(read, run).await? {
            RecoveryDisposition::Recovering
        } else {
            RecoveryDisposition::Normal
        };
    }

    // A timer claim held by a prior daemon instance must be reclaimed before
    // the run can be `Normal` (`specs/recovery-table.md`, recovery actions
    // outside the run-by-effect matrix).
    if disposition == RecoveryDisposition::Normal
        && timers
            .iter()
            .any(|timer| is_stale_claimed_timer(timer, daemon_epoch))
    {
        disposition = RecoveryDisposition::Recovering;
    }

    Ok(disposition)
}

/// Highest-precedence class across a run's in-flight effects.
///
/// Settled effects (`Committed`, `Failed`, `Cancelled`) are equivalent to no
/// effect for recovery and add no ambiguity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectClass {
    None,
    Prepared,
    Claimed,
    Acknowledged,
    DispatchedSafe,
    DispatchedUnsafe,
    Unknown,
}

impl EffectClass {
    /// Reduces the persisted effects to the winning class, or `None` when a
    /// persisted effect state is not a known value (never a default).
    fn aggregate(effects: &[EffectRow]) -> Option<Self> {
        let mut class = Self::None;
        for effect in effects {
            let next = match effect.state {
                EffectState::Unspecified => return None,
                EffectState::Prepared => Self::Prepared,
                EffectState::Claimed => Self::Claimed,
                EffectState::Acknowledged => Self::Acknowledged,
                EffectState::Dispatched => {
                    if dispatched_is_unsafe(effect) {
                        Self::DispatchedUnsafe
                    } else {
                        Self::DispatchedSafe
                    }
                }
                EffectState::Unknown => Self::Unknown,
                EffectState::Committed | EffectState::Failed | EffectState::Cancelled => continue,
            };
            if next.precedence() > class.precedence() {
                class = next;
            }
        }
        Some(class)
    }

    /// Match precedence: unknown effects beat reconciliation, which beats the
    /// ordinary rows (D5, class order B > A > C > D).
    const fn precedence(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Prepared => 1,
            Self::Acknowledged => 2,
            Self::Claimed => 3,
            Self::DispatchedSafe => 4,
            Self::DispatchedUnsafe => 5,
            Self::Unknown => 6,
        }
    }

    /// Ordinary classes are the ones whose matrix rows assume the run's
    /// frozen references resolve, so Matrix D may override them.
    const fn is_ordinary(self) -> bool {
        matches!(
            self,
            Self::None | Self::Prepared | Self::Claimed | Self::Acknowledged
        )
    }
}

/// Returns true for a `Dispatched` effect that can be neither reconciled nor
/// safely retried, which the matrix blocks as an unknown effect.
fn dispatched_is_unsafe(effect: &EffectRow) -> bool {
    let reconciliation_is_impossible = matches!(
        effect.reconciliation_semantics,
        ReconciliationSemantics::Impossible | ReconciliationSemantics::UnknownReconciliation
    );
    let retry_is_unsafe = matches!(
        effect.idempotency_semantics,
        IdempotencySemantics::NotIdempotent | IdempotencySemantics::UnknownIdempotency
    );
    reconciliation_is_impossible && retry_is_unsafe
}

/// Assigns the disposition a matrix row names, or `None` when no row covers
/// the pair.
///
/// Matrix C (terminal owning run) is unreachable here: terminal runs are never
/// enumerated, so a terminal state reaching classification is treated as
/// unmapped rather than guessed. The `WaitingChild` arm is a placeholder that
/// [`classify`] refines with the declared-condition check.
fn matrix(run_state: RunState, class: EffectClass) -> Option<RecoveryDisposition> {
    use EffectClass as E;
    use RecoveryDisposition as D;
    use RunState as S;

    let disposition = match (run_state, class) {
        (S::Created | S::Ready, E::None) => D::Normal,
        (S::Running, E::None) => D::Recovering,
        (S::WaitingTool, E::None) => D::Normal,
        (S::WaitingChild, E::None) => D::Normal,
        (S::WaitingHuman, E::None) => D::Normal,
        (
            S::Suspended,
            E::None | E::Prepared | E::Claimed | E::Acknowledged | E::DispatchedSafe,
        ) => D::RequiresHumanDecision,
        (S::Running | S::WaitingTool, E::Prepared) => D::Normal,
        (S::Running | S::WaitingTool | S::Cancelling, E::Claimed | E::Acknowledged) => {
            D::Recovering
        }
        (S::Running | S::WaitingTool | S::Cancelling, E::DispatchedSafe) => D::NeedsReconciliation,
        (
            S::Running | S::WaitingTool | S::Cancelling | S::Suspended,
            E::DispatchedUnsafe | E::Unknown,
        ) => D::BlockedUnknownEffect,
        (S::Cancelling, E::None | E::Prepared) => D::Recovering,
        _ => return None,
    };
    Some(disposition)
}

/// Loads the run's frozen environment and bindings when one is referenced.
async fn load_frozen_environment(
    read: &mut dyn KernelReadTxn,
    run: &RunRow,
) -> errors::Result<(Option<ResolvedEnvironmentRow>, Vec<ResolvedBindingRow>)> {
    let Some(environment_id) = run.resolved_environment_id else {
        return Ok((None, Vec::new()));
    };
    let environment = read.environments().get_environment(environment_id).await?;
    let bindings = if environment.is_some() {
        read.environments().get_bindings(environment_id).await?
    } else {
        Vec::new()
    };
    Ok((environment, bindings))
}

/// Matrix D: an in-flight `Prepared` or `Claimed` effect whose frozen adapter
/// id/version/digest cannot be resolved blocks the run; no replacement binding
/// is ever substituted (the frozen environment is immutable).
fn missing_resource(
    run: &RunRow,
    effects: &[EffectRow],
    environment: Option<&ResolvedEnvironmentRow>,
    bindings: &[ResolvedBindingRow],
) -> bool {
    if !matches!(
        run.state,
        RunState::Running | RunState::WaitingTool | RunState::Cancelling | RunState::Suspended
    ) {
        return false;
    }
    for effect in effects
        .iter()
        .filter(|effect| matches!(effect.state, EffectState::Prepared | EffectState::Claimed))
    {
        let Some(environment) = environment else {
            return true;
        };
        if environment.run_id != run.run_id {
            return true;
        }
        let bound = bindings.iter().any(|binding| {
            binding.adapter_id == effect.adapter_id
                && binding.adapter_version == effect.adapter_version
                && binding.adapter_digest == effect.adapter_digest
        });
        if !bound {
            return true;
        }
    }
    false
}

/// Returns true when every dependency edge targeting the waiting parent is
/// satisfied by its source run's persisted state. With no declared condition
/// edges the direct children must all be terminal.
async fn waiting_child_advanced(
    read: &mut dyn KernelReadTxn,
    run: &RunRow,
) -> errors::Result<bool> {
    let runs = read.runs().list_by_task(run.task_id).await?;
    let dependencies = read.graph().list_dependencies(run.task_id).await?;

    let mut declared = false;
    for dependency in dependencies
        .iter()
        .filter(|dependency| dependency.target_run_id == run.run_id)
    {
        declared = true;
        let source = runs
            .iter()
            .find(|row| row.run_id == dependency.source_run_id)
            .ok_or_else(|| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "waiting-child dependency source is missing",
                )
            })?;
        if !condition_met(dependency.dependency_condition, source.state) {
            return Ok(false);
        }
    }
    if declared {
        return Ok(true);
    }
    Ok(runs
        .iter()
        .filter(|row| row.parent_run_id == Some(run.run_id))
        .all(|child| is_terminal(child.state)))
}

/// A claimed timer is stale unless the current daemon epoch owns the claim;
/// recovery runs before any worker of the current epoch exists.
fn is_stale_claimed_timer(timer: &TimerRow, daemon_epoch: u64) -> bool {
    timer.state == TimerState::Claimed && timer.claim_daemon_epoch != Some(daemon_epoch)
}

/// Returns true for the in-flight effect states (`PREPARED`, `CLAIMED`,
/// `DISPATCHED`, `ACKNOWLEDGED`, `UNKNOWN`).
const fn is_in_flight(state: EffectState) -> bool {
    matches!(
        state,
        EffectState::Prepared
            | EffectState::Claimed
            | EffectState::Dispatched
            | EffectState::Acknowledged
            | EffectState::Unknown
    )
}

/// Builds the fail-closed error for a pair no matrix row assigns.
///
/// The message carries the offending identifiers and wire states; payload
/// bytes are never rendered.
fn unmapped(run: &RunRow, effect: Option<&EffectRow>) -> KernelError {
    let detail = match effect {
        Some(effect) => format!(
            "unmapped recovery combination: run {} state {} with effect {} state {}",
            run.run_id,
            run.state.to_wire(),
            effect.effect_id,
            effect.state.to_wire()
        ),
        None => format!(
            "unmapped recovery combination: run {} state {}",
            run.run_id,
            run.state.to_wire()
        ),
    };
    KernelError::new(ErrorCode::Internal, RetryClass::Never, detail)
}

/// Builds the `AgentRun` snapshot payload for a changed run.
fn run_payload(row: &RunRow) -> contract::AgentRun {
    contract::AgentRun {
        run_id: row.run_id.to_string(),
        task_id: row.task_id.to_string(),
        session_id: row.session_id.map(|id| id.to_string()).unwrap_or_default(),
        parent_run_id: row
            .parent_run_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        state: row.state.to_wire(),
        recovery: row.recovery.to_wire(),
        run_revision: row.run_revision,
        loop_epoch: row.loop_epoch,
        step_sequence: row.step_sequence,
        input_event_cursor: row.input_event_cursor.to_string(),
        cancellation_epoch: row.cancellation_epoch,
        resolved_environment_id: row
            .resolved_environment_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        output_ref: row.output_ref.clone().unwrap_or_default(),
        current_turn_id: row
            .current_turn_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{EffectClass, matrix};
    use domain::run::{RecoveryDisposition, RunState};

    #[test]
    fn matrix_covers_every_persisted_pair_it_names() {
        assert_eq!(
            matrix(RunState::Running, EffectClass::None),
            Some(RecoveryDisposition::Recovering)
        );
        assert_eq!(
            matrix(RunState::Suspended, EffectClass::DispatchedSafe),
            Some(RecoveryDisposition::RequiresHumanDecision)
        );
        assert_eq!(
            matrix(RunState::Created, EffectClass::Prepared),
            None,
            "no loop turn can run before binding"
        );
        assert_eq!(
            matrix(RunState::WaitingHuman, EffectClass::Acknowledged),
            None,
            "effects settle before waiting transitions commit"
        );
    }
}
