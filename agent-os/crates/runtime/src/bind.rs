//! `BindRun` — the internal command that persists a run's immutable
//! `ResolvedRunEnvironment`, links it, and transitions `Created -> Ready`.
//!
//! The Config/Runtime binder produces the `resolved_environment` payload;
//! this handler validates the run is still `Created` at the expected
//! revision, inserts the environment + bindings exactly once, emits
//! `RunReady`/`RunBound`/`RunStateChanged`, and replays an identical
//! resubmission without mutation. A bind is final: a different
//! environment for a bound run is `Conflict`.
#![forbid(unsafe_code)]

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{AdapterId, EnvironmentId, EventId, RunId};
use domain::resource::WorkspaceAccessMode;
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::{NewResolvedBinding, NewResolvedEnvironment, RunCas, RunPatch, RunRow};
use prost::Message;

use crate::resolved_environment::{EnvironmentPlan, freeze_environment};
use crate::{RuntimeDeps, decode_contract, invalid_field, required_id, stage_catalogued};

/// Fully-qualified command type of `BindRun`.
pub const CMD_BIND_RUN: &str = "agentos.spec.v1.BindRun";

/// Decoded binding instruction (the resolved environment the run freezes).
fn decode_environment(
    raw: contract::ResolvedRunEnvironment,
    now_ms: i64,
) -> errors::Result<EnvironmentPlan> {
    let environment_id = required_id::<EnvironmentId>("resolved_environment.id", &raw.id)?;
    let run_id = required_id::<RunId>("resolved_environment.run_id", &raw.run_id)?;
    let agent_spec = raw
        .agent_spec
        .ok_or_else(|| invalid_field("resolved_environment.agent_spec", "is required"))?;
    let agent_loop = raw
        .agent_loop
        .ok_or_else(|| invalid_field("resolved_environment.agent_loop", "is required"))?;
    let workspace_mode = WorkspaceAccessMode::from_wire(raw.workspace_mode)
        .map_err(|_| invalid_field("resolved_environment.workspace_mode", "is not a known mode"))?;
    let workspace_uri = raw
        .workspace_uri
        .map(|uri| uri.uri)
        .filter(|uri| !uri.is_empty());
    let bindings = raw
        .bindings
        .iter()
        .map(|binding| {
            let adapter = binding.adapter.as_ref().ok_or_else(|| {
                invalid_field("resolved_environment.bindings.adapter", "is required")
            })?;
            Ok(NewResolvedBinding {
                port_id: binding.port_id.clone(),
                adapter_id: required_id::<AdapterId>(
                    "resolved_environment.bindings.adapter.id",
                    &adapter.id,
                )?,
                adapter_version: adapter.version.clone(),
                adapter_digest: adapter.digest.clone(),
                capabilities: serde_json::to_vec(&binding.capabilities).map_err(|error| {
                    KernelError::new(
                        ErrorCode::Internal,
                        RetryClass::Never,
                        "binding capabilities do not encode",
                    )
                    .with_source(error)
                })?,
            })
        })
        .collect::<errors::Result<Vec<_>>>()?;
    let environment = NewResolvedEnvironment {
        environment_id,
        run_id,
        agent_spec_id: required_id("resolved_environment.agent_spec.id", &agent_spec.id)?,
        agent_spec_version: agent_spec.version,
        agent_spec_digest: agent_spec.digest,
        agent_loop_id: agent_loop.id,
        agent_loop_version: agent_loop.version,
        agent_loop_digest: agent_loop.digest,
        config_generation_id: required_id(
            "resolved_environment.config_generation_id",
            &raw.config_generation_id,
        )?,
        workspace_uri,
        workspace_base_revision: if raw.workspace_base_revision.is_empty() {
            None
        } else {
            Some(raw.workspace_base_revision)
        },
        workspace_mode,
        model_provider: if raw.model_provider.is_empty() {
            None
        } else {
            Some(raw.model_provider)
        },
        model_id: if raw.model_id.is_empty() {
            None
        } else {
            Some(raw.model_id)
        },
        model_parameters: if raw.model_parameters_json.is_empty() {
            None
        } else {
            Some(raw.model_parameters_json)
        },
        kernel_version: raw.kernel_version,
        protocol_versions: raw.protocol_versions.join(",").into_bytes(),
        capability_grant_ids: raw.capability_grant_ids.join("\n").into_bytes(),
        approval_request_ids: raw.approval_request_ids.join("\n").into_bytes(),
        created_at_ms: now_ms,
    };
    let generation_id = environment.config_generation_id;
    Ok(EnvironmentPlan {
        environment,
        bindings,
        generation_id,
    })
}

/// Handler for `agentos.spec.v1.BindRun`.
pub struct BindRunHandler {
    deps: RuntimeDeps,
}

impl BindRunHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for BindRunHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::BindRun>("BindRun", &payload)?;
        let run_id = required_id::<RunId>("run_id", &raw.run_id)?;
        let raw_env = raw
            .resolved_environment
            .ok_or_else(|| invalid_field("resolved_environment", "is required"))?;
        if raw_env.run_id != raw.run_id {
            return Err(invalid_field(
                "resolved_environment.run_id",
                "must equal the bound run",
            ));
        }
        if !raw.config_generation_id.is_empty()
            && raw.config_generation_id != raw_env.config_generation_id
        {
            return Err(invalid_field(
                "config_generation_id",
                "must equal the environment's generation",
            ));
        }
        let environment_id = required_id::<EnvironmentId>("resolved_environment.id", &raw_env.id)?;
        let plan = decode_environment(raw_env, self.deps.now_unix_ms())?;

        let run = txn
            .runs()
            .get(run_id)
            .await?
            .ok_or_else(|| not_found("run not found"))?;
        if run.resolved_environment_id == Some(environment_id) {
            // Identical replay of a committed bind: return the stored
            // outcome without touching state.
            return Ok(CommandOutcome {
                code: OutcomeCode::Ok,
                payload: environment_id.to_string().into_bytes(),
            });
        }
        if run.resolved_environment_id.is_some() {
            return Err(conflict("run is already bound to a different environment"));
        }
        if run.state != RunState::Created {
            return Err(failed_precondition("run is not in Created"));
        }
        if run.run_revision != raw.expected_run_revision {
            return Err(conflict("expected_run_revision is stale"));
        }

        freeze_environment(txn, plan, run_id, run.run_revision).await?;
        let bound = txn.runs().get(run_id).await?.ok_or_else(internal)?;
        let moved = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: bound.run_revision,
                    state: Some(RunState::Created),
                    cancellation_epoch: None,
                },
                RunPatch {
                    state: Some(RunState::Ready),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await?;
        if !moved {
            return Err(conflict("run moved during bind"));
        }
        let ready = txn.runs().get(run_id).await?.ok_or_else(internal)?;
        let payload = run_payload(&ready).encode_to_vec();
        for event_type in ["RunReady", "RunBound", "RunStateChanged"] {
            stage_catalogued(
                txn,
                EventId::new(self.deps.ids.as_ref()),
                event_type,
                StreamKey::run(run_id),
                payload.clone(),
                ctx.correlation_id.clone(),
                None,
            )
            .await?;
        }
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: environment_id.to_string().into_bytes(),
        })
    }
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

fn not_found(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::NotFound, RetryClass::Never, detail)
}

fn conflict(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Conflict, RetryClass::Never, detail)
}

fn failed_precondition(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, detail)
}

fn internal() -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        "bind invariant broken",
    )
}
