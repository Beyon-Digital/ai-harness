//! `ResolveUnknownEffect`: operator-gated resolution of `Unknown` effects.
//!
//! An `Unknown` effect is never retried by the kernel. This command resolves
//! it explicitly, and only after proving the caller's delegation chain holds
//! `effect.resolve_unknown` — or, when policy requires an approval, after a
//! matching approved `approval_requests` row exists. The settled transition
//! and `EffectReconciled` audit event are staged inside the command
//! transaction.

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::effect::EffectState;
use domain::generated::contract;
use domain::ids::{ApprovalRequestId, EffectId};
use domain::security::ApprovalState;
use effects::reconcile::{Resolution, apply_resolution};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{
    Capability, CapabilityAction, CapabilityFamily, DelegationChain, ScopedTarget, load_chain,
    load_grant_scopes,
};
use kernel_store::KernelTxn;
use kernel_store::models::EffectRow;
use permissions::{Decision, PermissionRequest, evaluate};

use crate::{RuntimeDeps, decode_contract, optional_id, required_id};

/// Fully-qualified command type of `ResolveUnknownEffect`.
pub const CMD_RESOLVE_UNKNOWN_EFFECT: &str = "agentos.spec.v1.ResolveUnknownEffect";

/// Canonical operation token recorded on approval requests for this command.
const RESOLVE_OPERATION: &str = "effect.resolve_unknown";

/// Handles `ResolveUnknownEffect`.
pub struct ResolveUnknownEffectHandler {
    deps: RuntimeDeps,
}

impl ResolveUnknownEffectHandler {
    /// Builds the handler with shared runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for ResolveUnknownEffectHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let request =
            decode_contract::<contract::ResolveUnknownEffect>("ResolveUnknownEffect", &payload)?;
        let effect_id: EffectId = required_id("effect_id", &request.effect_id)?;
        let resolution = Resolution::parse(&request.action)?;
        let row = txn.effects().get(effect_id).await?.ok_or_else(|| {
            KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "effect does not exist",
            )
        })?;
        let expected = EffectState::from_wire(request.expected_effect_state).map_err(|_| {
            KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "expected_effect_state is not a canonical effect state",
            )
        })?;
        if row.state != expected {
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "expected_effect_state does not match the persisted state",
            ));
        }
        if row.state != EffectState::Unknown {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "only an Unknown effect may be resolved",
            ));
        }
        self.authorize(ctx, txn, &row, &request).await?;
        let env = effects::EffectEnv {
            ids: self.deps.ids.as_ref(),
            clock: self.deps.clock.as_ref(),
            correlation_id: ctx.correlation_id.clone(),
            causation_id: None,
        };
        let row = apply_resolution(
            txn,
            &env,
            effect_id,
            resolution,
            if request.result_ref.is_empty() {
                None
            } else {
                Some(request.result_ref)
            },
            &request.reason,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: row.effect_id.to_string().into_bytes(),
        })
    }
}

impl ResolveUnknownEffectHandler {
    /// Capability + approval gate for the resolution.
    async fn authorize(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        row: &EffectRow,
        request: &contract::ResolveUnknownEffect,
    ) -> errors::Result<()> {
        let capability =
            Capability::new(CapabilityFamily::Effect, CapabilityAction::ResolveUnknown);
        let chain = self.load_chain(ctx, txn).await?;
        let grant_ids: Vec<_> = chain
            .hops
            .iter()
            .flat_map(|hop| hop.grant_ids.iter().copied())
            .collect();
        let grant_scopes = load_grant_scopes(txn, &grant_ids).await?;
        let decision = evaluate(&PermissionRequest {
            principal_id: ctx.principal_id,
            actor_id: ctx.actor_id,
            run_id: Some(row.run_id),
            chain,
            grant_scopes,
            tool_capabilities: None,
            capability,
            target: ScopedTarget::None,
            extension_digest: None,
            config_digest: None,
            now_ms: self.deps.now_unix_ms(),
        });
        match decision {
            Decision::Allow { .. } => Ok(()),
            // An approved operator request is the compensating control when
            // the delegation chain does not carry standing authority.
            Decision::Deny { .. } | Decision::RequireApproval { .. } => {
                self.verify_approval(txn, row, request).await
            }
        }
    }

    /// Loads the caller's delegation chain; absence fails closed.
    async fn load_chain(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
    ) -> errors::Result<DelegationChain> {
        match ctx.delegation_chain_id {
            Some(chain_id) => load_chain(txn, chain_id).await,
            None => Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "ResolveUnknownEffect requires a delegation chain",
            )),
        }
    }

    /// Verifies a cited approved request covers this effect's resolution.
    async fn verify_approval(
        &self,
        txn: &mut dyn KernelTxn,
        row: &EffectRow,
        request: &contract::ResolveUnknownEffect,
    ) -> errors::Result<()> {
        let approval_id: Option<ApprovalRequestId> =
            optional_id("approval_request_id", &request.approval_request_id)?;
        let approval_id = approval_id.ok_or_else(needs_approval)?;
        let approval = txn
            .security()
            .get_approval_request(approval_id)
            .await?
            .ok_or_else(needs_approval)?;
        let covered = approval.state == ApprovalState::Approved
            && approval.operation == RESOLVE_OPERATION
            && approval.run_id == Some(row.run_id)
            && approval.expires_at_ms > self.deps.now_unix_ms();
        if covered {
            Ok(())
        } else {
            Err(needs_approval())
        }
    }
}

fn needs_approval() -> KernelError {
    KernelError::new(
        ErrorCode::FailedPrecondition,
        RetryClass::Never,
        "ResolveUnknownEffect requires an approved approval request for this run",
    )
}
