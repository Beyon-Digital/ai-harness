//! Subtree cancellation: root epoch advance and eligible-run discovery.
//!
//! `cancel_subtree` performs the graph-side half of `CancelRun` (recipe G): it
//! compare-and-set increments the root's `cancellation_epoch` and stages
//! `CancellationEpochAdvanced` inside the caller's transaction, then walks the
//! known descendant closure through `runs.parent_run_id` (R3.6) and reports
//! every run whose state the caller must still transition (R5.1-R5.5, D6).
//!
//! The epoch bump is the spawn fence: child creation compares the caller's
//! `observed_parent_cancellation_epoch` inside its own transaction, so a spawn
//! that observed the pre-advance value can never commit after this one (R5.3,
//! P3). The service itself changes no run state; the runtime state table stays
//! the only transition authority, and the runtime service applies it through
//! `run::transition` for each returned run id (R5.2, R5.4, R5.5).

use domain::generated::contract;
use domain::ids::{EventId, EventStreamKey, RunId};
use domain::provider::SystemIdProvider;
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::outbox::{DraftEvent, stage};
use events::{CatalogClassificationPolicy, ClassificationPolicy, StreamKey};
use kernel_store::KernelTxn;
use kernel_store::models::{RunCas, RunPatch, RunRow};
use prost::Message;

use crate::repository::{descendants, load_run};

/// Catalogued event type staged when the root's cancellation epoch advances.
const CANCELLATION_EPOCH_ADVANCED: &str = "CancellationEpochAdvanced";

/// Advances the root's cancellation epoch and reports the eligible subtree.
///
/// The epoch CAS fences on the root's revision, state, and observed epoch, so a
/// racing cancellation loses with `Conflict` and leaves the row untouched; on
/// success the revision increments exactly once and
/// `CancellationEpochAdvanced` is staged on the root's stream in the same
/// transaction (R5.1, R5.3, R5.4). The returned ids start with the root when it
/// is eligible and continue with the breadth-first descendant closure over
/// `runs.parent_run_id`; every returned run is non-terminal and not already
/// `Cancelling`, so the caller transitions each exactly once (R5.2, R5.5).
pub async fn cancel_subtree(
    txn: &mut dyn KernelTxn,
    root: RunId,
    _reason: &str,
    _now_ms: i64,
) -> errors::Result<Vec<RunId>> {
    let root_row = load_run(txn, root).await?;
    let advanced = advance_epoch(txn, &root_row).await?;
    stage_epoch_event(txn, &advanced).await?;

    let mut eligible = Vec::new();
    if is_eligible(advanced.state) {
        eligible.push(root);
    }
    for descendant in descendants(txn, root).await? {
        let row = txn.runs().get(descendant).await?.ok_or_else(|| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "descendant vanished during the cancellation walk",
            )
        })?;
        if is_eligible(row.state) {
            eligible.push(descendant);
        }
    }
    Ok(eligible)
}

/// Returns true when cancellation still has to move the run: every non-terminal
/// state except `Cancelling`, which is already draining (R5.2, R5.5).
const fn is_eligible(state: RunState) -> bool {
    matches!(
        state,
        RunState::Created
            | RunState::Ready
            | RunState::Running
            | RunState::WaitingTool
            | RunState::WaitingChild
            | RunState::WaitingHuman
            | RunState::Suspended
    )
}

/// Compare-and-set increments `cancellation_epoch` and bumps the revision once.
async fn advance_epoch(txn: &mut dyn KernelTxn, root: &RunRow) -> errors::Result<RunRow> {
    let updated = txn
        .runs()
        .cas_update(
            root.run_id,
            RunCas {
                run_revision: root.run_revision,
                state: Some(root.state),
                cancellation_epoch: Some(root.cancellation_epoch),
            },
            RunPatch {
                cancellation_epoch: Some(root.cancellation_epoch.saturating_add(1)),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await?;
    if !updated {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "cancellation epoch advance lost a revision race",
        ));
    }
    txn.runs().get(root.run_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "root run vanished after its epoch advance",
        )
    })
}

/// Stages the catalogued `CancellationEpochAdvanced` on the root's stream.
async fn stage_epoch_event(txn: &mut dyn KernelTxn, root: &RunRow) -> errors::Result<()> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy
        .minimum(CANCELLATION_EPOCH_ADVANCED)
        .ok_or_else(uncatalogued)?;
    let retention = policy
        .default_retention(CANCELLATION_EPOCH_ADVANCED)
        .ok_or_else(uncatalogued)?;
    let stream_key =
        EventStreamKey::new(StreamKey::run(root.run_id).as_str().to_owned()).map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "catalogued stream key is not canonical",
            )
        })?;
    stage(
        txn,
        DraftEvent {
            event_id: EventId::new(&SystemIdProvider),
            stream_key,
            event_type: CANCELLATION_EPOCH_ADVANCED.to_owned(),
            payload: run_payload(root).encode_to_vec(),
            sensitivity,
            retention,
            correlation_id: None,
            causation_id: None,
        },
    )
    .await?;
    Ok(())
}

/// Builds the `AgentRun` snapshot payload for the advanced root.
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

fn uncatalogued() -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        "event type CancellationEpochAdvanced has no catalog entry",
    )
}
