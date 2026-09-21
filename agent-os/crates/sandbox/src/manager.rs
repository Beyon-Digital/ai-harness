//! Sandbox Manager (SBOX-001): tier resolution + workspace-lease-gated
//! T0 execution.
//!
//! Tier semantics are strict: a request for T1/T2/T3 resolves **only** a
//! registered adapter whose negotiated capabilities host that tier —
//! when none qualifies the request fails `CAPABILITY_UNSUPPORTED`-style
//! (`FailedPrecondition`). There is no T2 -> T0 fallback.
//!
//! The T0 adapter is the trusted local process. It is documented and
//! labelled `is_security_boundary: false` — callers and the frozen run
//! environment record exactly that.
#![forbid(unsafe_code)]

use adapter_registry::capabilities::SandboxTier;
use adapter_registry::resolver::{Candidate, PortRequirement, resolve};
use domain::ids::{LeaseId, RunId, WorkspaceId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::txn::KernelTxn;

use crate::local_process::{self, ExecSpec};
use workspace::coordinator;

fn manager_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// A live sandbox instance resolved for a run.
#[derive(Clone, Debug)]
pub struct SandboxInstance {
    /// The tier the run is actually executing under.
    pub tier: SandboxTier,
    /// Adapter kind label (`local-process-t0` for the MVP).
    pub kind: String,
    /// Negotiated capabilities recorded into the run environment.
    pub negotiated_capabilities: Vec<String>,
    /// T0 truth: never a security boundary on the local adapter.
    pub is_security_boundary: bool,
}

/// Resolve a sandbox for `tier`. `candidates` are the registry's
/// registered adapters decoded for the resolver; when the registry has
/// no qualifying adapter the call fails closed.
pub fn resolve_sandbox(
    tier: SandboxTier,
    candidates: &[Candidate],
) -> errors::Result<SandboxInstance> {
    if tier == SandboxTier::T0 {
        return Ok(SandboxInstance {
            tier,
            kind: "local-process-t0".to_owned(),
            negotiated_capabilities: local_process::CAPABILITIES
                .iter()
                .map(|c| (*c).to_owned())
                .collect(),
            is_security_boundary: false,
        });
    }
    let requirement = PortRequirement {
        port_id: "sandbox".to_owned(),
        port_version: 1,
        required_capabilities: Vec::new(),
        sandbox_tier: tier,
        pin_adapter_id: None,
        require_conformance_passed: true,
    };
    let resolved = resolve(&requirement, candidates, &[]).map_err(|_| {
        manager_error(
            ErrorCode::FailedPrecondition,
            format!("capability unsupported: no adapter passes sandbox tier {tier:?}"),
        )
    })?;
    Ok(SandboxInstance {
        tier,
        kind: format!("{}@{}", resolved.adapter_id, resolved.version),
        negotiated_capabilities: resolved.negotiated_capabilities,
        // Only a conformance-passed adapter that hosts the tier may claim
        // boundary status; the resolver already gate-kept that.
        is_security_boundary: tier >= SandboxTier::T2,
    })
}

/// Execute `spec` inside a workspace the caller holds authority on.
///
/// `lease_id`/`epoch`/`owner` are verified against the durable lease row
/// before spawn — a revoked or transferred lease stops the coordinator-
/// mediated exec path immediately.
pub async fn exec_in_workspace(
    txn: &mut dyn KernelTxn,
    spec: &ExecSpec,
    workspace_id: WorkspaceId,
    lease_id: LeaseId,
    owner: RunId,
    lease_epoch: u64,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> errors::Result<local_process::ExecOutcome> {
    coordinator::verify_write_authority(txn, lease_id, workspace_id, owner, lease_epoch)
        .await
        .map_err(|e| {
            manager_error(
                e.code(),
                format!("workspace lease gate denied exec: {}", e.message()),
            )
        })?;
    local_process::exec(spec, cancel).await
}
