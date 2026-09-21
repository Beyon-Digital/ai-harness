//! ResolvedRunEnvironment (CFG-003): resolve a run's exact bindings once
//! and freeze them before the run executes.
//!
//! The environment row + `resolved_bindings` rows are inserted in the
//! same transaction that CAS-patches the run's
//! `resolved_environment_id`; schema triggers make both tables
//! update- and delete-proof afterwards, so historical audit never
//! consults mutable registry/config state.
#![forbid(unsafe_code)]

use adapter_registry::capabilities::SandboxTier;
use adapter_registry::resolver::{Candidate, PortRequirement, ResolvedAdapter, resolve};
use config_engine::model::{self, ConfigDocument};
use config_engine::profile;
use domain::ids::{ConfigGenerationId, EnvironmentId, RunId};
use domain::provider::IdProvider;
use domain::resource::WorkspaceAccessMode;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewResolvedBinding, NewResolvedEnvironment};
use kernel_store::txn::KernelTxn;

fn env_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Kernel build label frozen into every environment.
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the resolver produced for a run before persistence.
#[derive(Clone, Debug)]
pub struct EnvironmentPlan {
    /// Persisted environment row content.
    pub environment: NewResolvedEnvironment,
    /// Registry-resolved port bindings (builtins excluded — their
    /// identity is already carried by the environment columns).
    pub bindings: Vec<NewResolvedBinding>,
    /// The config generation read when the plan was made.
    pub generation_id: ConfigGenerationId,
}

/// Resolve `profile_name` under the active generation and build the
/// environment + bindings for `run` — nothing persisted yet.
///
/// Fails `FailedPrecondition` when a required port has no resolvable
/// adapter (missing required adapter fails run start *before* any
/// execution-facing side effect).
#[allow(clippy::too_many_arguments)]
pub async fn plan_environment(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    run: RunId,
    profile_name: &str,
    agent_spec: &crate::create_run::AgentSpecRef,
    agent_loop: &ResolvedAdapter,
    workspace_uri: Option<String>,
    workspace_base_revision: Option<String>,
    workspace_mode: WorkspaceAccessMode,
    capability_grant_ids: Vec<u8>,
    approval_request_ids: Vec<u8>,
    now_ms: i64,
) -> errors::Result<EnvironmentPlan> {
    let active = txn.config().get_active().await?.ok_or_else(|| {
        env_error(
            ErrorCode::FailedPrecondition,
            "no active config generation — cannot resolve a run environment",
        )
    })?;
    let generation = txn
        .config()
        .get_generation(active.generation_id)
        .await?
        .ok_or_else(|| env_error(ErrorCode::Internal, "active generation row missing"))?;
    let doc: ConfigDocument = model::parse_document(
        std::str::from_utf8(&generation.document)
            .map_err(|_| env_error(ErrorCode::Internal, "generation document not utf-8"))?,
    )?;
    let profile = profile::resolve_profile(&doc, profile_name)?;

    // Registry-backed ports resolve through the deterministic resolver.
    let candidates: Vec<Candidate> = txn
        .adapters()
        .list_registrations()
        .await?
        .into_iter()
        .map(Candidate::decode)
        .collect::<errors::Result<Vec<_>>>()?;
    let mut bindings = Vec::new();
    for (slot, binding) in &profile.bindings {
        if config_engine::generations::BUILTIN_ADAPTERS
            .iter()
            .any(|b| binding.starts_with(b))
        {
            continue;
        }
        let (name, version) = binding
            .split_once('@')
            .map(|(n, v)| (n, v.parse::<u32>().unwrap_or(1)))
            .unwrap_or((binding.as_str(), 1));
        let adapter_id = name.parse::<domain::ids::AdapterId>().map_err(|_| {
            env_error(
                ErrorCode::InvalidArgument,
                "binding id is not an adapter id",
            )
        })?;
        let req = PortRequirement {
            port_id: slot.clone(),
            port_version: version,
            required_capabilities: Vec::new(),
            sandbox_tier: SandboxTier::T0,
            pin_adapter_id: Some(adapter_id),
            require_conformance_passed: false,
        };
        let resolved = resolve(&req, &candidates, &[]).map_err(|_| {
            env_error(
                ErrorCode::FailedPrecondition,
                format!("required adapter '{binding}' for port '{slot}' is unavailable"),
            )
        })?;
        bindings.push(NewResolvedBinding {
            port_id: slot.clone(),
            adapter_id: resolved.adapter_id,
            adapter_version: resolved.version,
            adapter_digest: resolved.bundle_digest,
            capabilities: serde_json::to_vec(&resolved.negotiated_capabilities)
                .map_err(|e| env_error(ErrorCode::Internal, format!("caps encode: {e}")))?,
        });
    }

    let environment = NewResolvedEnvironment {
        environment_id: EnvironmentId::new(ids),
        run_id: run,
        agent_spec_id: agent_spec.agent_spec_id,
        agent_spec_version: agent_spec.version.clone(),
        agent_spec_digest: agent_spec.digest.clone(),
        agent_loop_id: agent_loop.adapter_id.to_string(),
        agent_loop_version: agent_loop.version.clone(),
        agent_loop_digest: agent_loop.bundle_digest.clone(),
        config_generation_id: active.generation_id,
        workspace_uri,
        workspace_base_revision,
        workspace_mode,
        model_provider: profile.bindings.get("model_provider").cloned(),
        model_id: None,
        model_parameters: None,
        kernel_version: KERNEL_VERSION.to_owned(),
        protocol_versions: serde_json::to_vec(&[1u32])
            .map_err(|e| env_error(ErrorCode::Internal, format!("protocol versions: {e}")))?,
        capability_grant_ids,
        approval_request_ids,
        created_at_ms: now_ms,
    };
    Ok(EnvironmentPlan {
        environment,
        bindings,
        generation_id: active.generation_id,
    })
}

/// Persist the plan + bind it to the run in one transaction: insert the
/// environment row, insert its bindings, then CAS the run's
/// `resolved_environment_id` (expecting the run's current revision).
/// A stale `expected_run_revision` loses with `Conflict`.
pub async fn freeze_environment(
    txn: &mut dyn KernelTxn,
    plan: EnvironmentPlan,
    run: RunId,
    expected_run_revision: u64,
) -> errors::Result<EnvironmentId> {
    let environment_id = plan.environment.environment_id;
    txn.environments()
        .insert_environment(plan.environment)
        .await?;
    txn.environments()
        .insert_bindings(environment_id, plan.bindings)
        .await?;
    let moved = txn
        .runs()
        .cas_update(
            run,
            kernel_store::models::RunCas {
                run_revision: expected_run_revision,
                state: None,
                cancellation_epoch: None,
            },
            kernel_store::models::RunPatch {
                resolved_environment_id: Some(environment_id),
                bump_revision: true,
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(env_error(
            ErrorCode::Conflict,
            "run moved during environment binding",
        ));
    }
    Ok(environment_id)
}
