//! `ClaimReadyRun`: fenced, expiring ownership of a ready run.
//!
//! Eligibility is decided entirely inside the caller's transaction: the run
//! must be `Ready` with recovery `Normal`, must not carry an unexpired claim,
//! and every dependency edge into it must be satisfied. The winning update is
//! a single CAS that writes the owner, token, expiry, and daemon epoch, bumps
//! the revision, and stages `RunClaimed` plus `RunStarted` (R4.2-R4.6).

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{EventId, RunId};
use domain::provider::{IdProvider, SystemIdProvider};
use domain::run::{RecoveryDisposition, RunState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::{ClaimPatch, RunCas, RunPatch, RunRow};
use prost::Message;
use run_graph::readiness::dependencies_satisfied;
use run_graph::repository::{has_live_claim, load_run};

use crate::{RuntimeDeps, decode_contract, invalid_field, required_id, stage_catalogued};

/// Claims a ready run for `owner`, fencing it for `ttl_ms`, using system-minted
/// identifiers.
///
/// Service-level convenience for callers that do not carry an [`IdProvider`];
/// command handlers call [`claim_with_ids`] with the injected
/// [`RuntimeDeps::ids`](crate::RuntimeDeps::ids) so deterministic tests control
/// the claim token and event ids.
pub async fn claim(
    txn: &mut dyn KernelTxn,
    run_id: RunId,
    owner: String,
    ttl_ms: u64,
    now_ms: i64,
    daemon_epoch: u64,
) -> errors::Result<()> {
    claim_with_ids(
        txn,
        run_id,
        owner,
        ttl_ms,
        now_ms,
        daemon_epoch,
        &SystemIdProvider,
    )
    .await
}

/// Claims a ready run for `owner`, fencing it for `ttl_ms`, drawing the claim
/// token and both event ids from `ids`.
///
/// Rejections never mutate: a non-`Ready` state or a live unexpired claim is
/// `Conflict`; a non-`Normal` recovery disposition or an unmet dependency is
/// `FailedPrecondition`. An expired claim does not block reclamation, but the
/// disposition gate still applies (R4.5). The winning CAS also fences the
/// update on the observed revision and `Ready` state, so racing claimants
/// serialize and exactly one commits.
pub async fn claim_with_ids(
    txn: &mut dyn KernelTxn,
    run_id: RunId,
    owner: String,
    ttl_ms: u64,
    now_ms: i64,
    daemon_epoch: u64,
    ids: &dyn IdProvider,
) -> errors::Result<()> {
    let current = load_run(txn, run_id).await?;
    if current.state != RunState::Ready {
        return Err(conflict("run is not ready to be claimed"));
    }
    if current.recovery != RecoveryDisposition::Normal {
        return Err(failed_precondition(
            "run recovery disposition does not permit claiming",
        ));
    }
    if has_live_claim(&current, now_ms) {
        return Err(conflict("run already carries a live claim"));
    }
    if !dependencies_satisfied(txn, run_id).await? {
        return Err(failed_precondition("run dependencies are not satisfied"));
    }

    let expires_unix_ms = now_ms.saturating_add(i64::try_from(ttl_ms).unwrap_or(i64::MAX));
    let updated = txn
        .runs()
        .cas_update(
            run_id,
            RunCas {
                run_revision: current.run_revision,
                state: Some(RunState::Ready),
                cancellation_epoch: None,
            },
            RunPatch {
                state: Some(RunState::Running),
                claim: Some(ClaimPatch {
                    owner,
                    token: fresh_token(ids),
                    expires_unix_ms,
                    daemon_epoch,
                }),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await?;
    if !updated {
        return Err(conflict("run claim lost a revision race"));
    }
    let claimed = txn.runs().get(run_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "run vanished after its claim",
        )
    })?;

    let payload = run_payload(&claimed).encode_to_vec();
    stage_catalogued(
        txn,
        EventId::new(ids),
        "RunClaimed",
        StreamKey::run(run_id),
        payload.clone(),
        None,
        None,
    )
    .await?;
    stage_catalogued(
        txn,
        EventId::new(ids),
        "RunStarted",
        StreamKey::run(run_id),
        payload,
        None,
        None,
    )
    .await?;
    Ok(())
}

/// Mints the claim token from the injected provider.
///
/// The SQLite `claim_token` column carries a non-negative `i64`, so the
/// UUIDv7's low 63 bits are kept; those bits are random, which is what the
/// fence needs.
fn fresh_token(ids: &dyn IdProvider) -> u64 {
    (ids.new_uuid_v7().as_u128() & i64::MAX as u128) as u64
}

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

/// Handler for `agentos.spec.v1.ClaimReadyRun`.
pub struct ClaimReadyRunHandler {
    deps: RuntimeDeps,
}

impl ClaimReadyRunHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for ClaimReadyRunHandler {
    async fn handle(
        &self,
        _ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::ClaimReadyRun>("ClaimReadyRun", &payload)?;
        let run_id = required_id::<RunId>("run_id", &raw.run_id)?;
        if raw.claim_owner.trim().is_empty() {
            return Err(invalid_field("claim_owner", "must not be empty"));
        }
        let now_ms = self.deps.now_unix_ms();
        let daemon_epoch = txn.context().daemon_epoch;
        claim_with_ids(
            txn,
            run_id,
            raw.claim_owner,
            raw.claim_ttl_ms,
            now_ms,
            daemon_epoch,
            self.deps.ids.as_ref(),
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: run_id.to_string().into_bytes(),
        })
    }
}

fn conflict(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Conflict, RetryClass::Never, detail)
}

fn failed_precondition(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, detail)
}
