//! `CancelRun`: epoch advance and subtree transition.
//!
//! The command advances the root's cancellation epoch (the spawn fence) and
//! transitions every eligible run the run-graph subtree walk reports through
//! [`transition`], so the normative state table and revision accounting stay
//! authoritative (R5.1-R5.5, D6). Never-started runs (`Created`, `Ready`) end
//! `Cancelled`; started non-terminal runs end `Cancelling`; terminal and
//! already-`Cancelling` runs are never touched (R5.2, R5.5). Each changed run
//! gets one catalogued run-state event in the same transaction (R5.4).

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{EventId, RunId};
use domain::provider::SystemIdProvider;
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::RunRow;
use prost::Message;
use run_graph::cancellation::cancel_subtree;

use crate::run::transition;
use crate::state::REASON_CANCELLATION_REQUESTED;
use crate::{RuntimeDeps, decode_contract, invalid_field, required_id, stage_catalogued};

/// Outcome of one cancellation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancellationReport {
    /// Run the epoch advance was applied to.
    pub root: RunId,
    /// Committed cancellation epoch of `root` after the advance.
    pub cancellation_epoch: u64,
    /// Runs transitioned by this request, in the order they were visited.
    pub changed: Vec<RunId>,
}

/// Advances the epoch of `run_id` and transitions its eligible subtree.
///
/// `cancel_subtree` owns the epoch CAS, the epoch event, and the ancestry walk;
/// this service applies the RUN-001 table with [`transition`] so revisions and
/// terminal reasons stay authoritative. Never-started runs cancel directly:
/// `Created -> Cancelled` is the table's cancellation-only edge (it avoids a
/// phantom `Ready` without a resolved environment) and `Ready -> Cancelled` has
/// always been allowed, so every changed run commits exactly one transition.
/// The committed revision increments once per transition plus once for the
/// epoch advance (R5.4).
///
/// When `expected_run_revision` is `Some`, the epoch CAS requires that exact
/// revision; a stale expectation conflicts before any epoch advance or staged
/// event, so two concurrent cancels sharing one expectation commit at most
/// once. `None` fences on the freshly loaded revision instead.
pub async fn cancel(
    txn: &mut dyn KernelTxn,
    run_id: RunId,
    expected_run_revision: Option<u64>,
    reason: &str,
    now_ms: i64,
) -> errors::Result<CancellationReport> {
    let eligible = cancel_subtree(txn, run_id, expected_run_revision, reason, now_ms).await?;

    let mut changed = Vec::new();
    for candidate in eligible {
        let row = txn.runs().get(candidate).await?.ok_or_else(|| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "eligible run vanished during cancellation",
            )
        })?;
        let (final_row, event_type) = match row.state.cancellation_target() {
            Some(RunState::Cancelled) => (
                transition(
                    txn,
                    candidate,
                    row.run_revision,
                    RunState::Cancelled,
                    terminal_reason(reason),
                    now_ms,
                )
                .await?,
                "RunCancelled",
            ),
            Some(RunState::Cancelling) => (
                transition(
                    txn,
                    candidate,
                    row.run_revision,
                    RunState::Cancelling,
                    Some(REASON_CANCELLATION_REQUESTED.to_owned()),
                    now_ms,
                )
                .await?,
                "RunStateChanged",
            ),
            _ => continue,
        };
        stage_run_event(txn, &final_row, event_type).await?;
        changed.push(candidate);
    }

    let root_row = txn.runs().get(run_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "root run vanished during cancellation",
        )
    })?;
    Ok(CancellationReport {
        root: run_id,
        cancellation_epoch: root_row.cancellation_epoch,
        changed,
    })
}

/// Caller reasons become terminal reasons; a blank reason stays canonical.
fn terminal_reason(reason: &str) -> Option<String> {
    if reason.trim().is_empty() {
        None
    } else {
        Some(reason.to_owned())
    }
}

/// Stages one catalogued run-state event on the changed run's stream.
async fn stage_run_event(
    txn: &mut dyn KernelTxn,
    row: &RunRow,
    event_type: &str,
) -> errors::Result<()> {
    stage_catalogued(
        txn,
        EventId::new(&SystemIdProvider),
        event_type,
        StreamKey::run(row.run_id),
        run_payload(row).encode_to_vec(),
        None,
        None,
    )
    .await
    .map(|_| ())
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

/// Handler for `agentos.spec.v1.CancelRun`.
pub struct CancelRunHandler {
    deps: RuntimeDeps,
}

impl CancelRunHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for CancelRunHandler {
    async fn handle(
        &self,
        _ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::CancelRun>("CancelRun", &payload)?;
        let run_id = required_id::<RunId>("run_id", &raw.run_id)?;
        if raw.reason.trim().is_empty() {
            return Err(invalid_field("reason", "must not be empty"));
        }
        let expected_run_revision =
            (raw.expected_run_revision != 0).then_some(raw.expected_run_revision);
        let report = cancel(
            txn,
            run_id,
            expected_run_revision,
            &raw.reason,
            self.deps.now_unix_ms(),
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: report.root.to_string().into_bytes(),
        })
    }
}
