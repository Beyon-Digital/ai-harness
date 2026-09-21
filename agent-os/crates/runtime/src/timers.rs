//! `ScheduleTimer` / `CancelTimer`: thin handlers over the scheduler crate.

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{RunId, TimerId};
use kernel_store::KernelTxn;
use scheduler::{ScheduleRequest, SchedulerEnv};

use crate::{RuntimeDeps, decode_contract, optional_id, required_id};

fn env<'a>(deps: &'a RuntimeDeps, ctx: &'a CommandContext) -> SchedulerEnv<'a> {
    SchedulerEnv {
        ids: deps.ids.as_ref(),
        clock: deps.clock.as_ref(),
        correlation_id: ctx.correlation_id.clone(),
        causation_id: None,
    }
}

/// Handler for `agentos.spec.v1.ScheduleTimer`.
pub struct ScheduleTimerHandler {
    deps: RuntimeDeps,
}

impl ScheduleTimerHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for ScheduleTimerHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::ScheduleTimer>("ScheduleTimer", &payload)?;
        let row = scheduler::schedule(
            txn,
            &env(&self.deps, ctx),
            ScheduleRequest {
                timer_id: optional_id::<TimerId>("timer_id", &raw.timer_id)?,
                run_id: optional_id::<RunId>("run_id", &raw.run_id)?,
                timer_kind: raw.timer_kind,
                due_at_ms: raw.due_at_ms,
                payload: raw.payload,
            },
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: row.timer_id.to_string().into_bytes(),
        })
    }
}

/// Handler for `agentos.spec.v1.CancelTimer`.
pub struct CancelTimerHandler {
    deps: RuntimeDeps,
}

impl CancelTimerHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for CancelTimerHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::CancelTimer>("CancelTimer", &payload)?;
        let timer_id = required_id::<TimerId>("timer_id", &raw.timer_id)?;
        scheduler::cancel(txn, &env(&self.deps, ctx), timer_id, raw.expected_version).await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: timer_id.to_string().into_bytes(),
        })
    }
}
