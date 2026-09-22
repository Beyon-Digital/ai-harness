//! Command handlers for the config-generation lifecycle.
//!
//! The daemon registers these so `agentctl config propose|test|activate|
//! rollback` flows through the Command Coordinator like every other
//! state change; each handler also stages its catalogued event on
//! `config/global` in the same transaction as the state mutation.

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use command_coordinator::handler::{
    CommandContext, CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode,
};
use domain::generated::contract;
use domain::ids::{ConfigGenerationId, EventId, EventStreamKey};
use domain::provider::IdProvider;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use events::outbox::{DraftEvent, stage};
use events::{CatalogClassificationPolicy, ClassificationPolicy};
use kernel_store::KernelTxn;
use prost::Message;

use crate::activate::{self, ActiveState};
use crate::generations;
use crate::model::document_digest;
use crate::schema::ServicesSection;

/// `agentos.spec.v1.ProposeConfigGeneration`.
pub const CMD_PROPOSE_CONFIG: &str = "agentos.spec.v1.ProposeConfigGeneration";
/// `agentos.spec.v1.MarkConfigTested`.
pub const CMD_MARK_CONFIG_TESTED: &str = "agentos.spec.v1.MarkConfigTested";
/// `agentos.spec.v1.ActivateConfigGeneration`.
pub const CMD_ACTIVATE_CONFIG: &str = "agentos.spec.v1.ActivateConfigGeneration";
/// `agentos.spec.v1.RollbackConfigGeneration`.
pub const CMD_ROLLBACK_CONFIG: &str = "agentos.spec.v1.RollbackConfigGeneration";

/// Dependencies shared by the config command handlers.
#[derive(Clone)]
pub struct ConfigDeps {
    /// Identifier source for staged outbox events.
    pub ids: Arc<dyn IdProvider>,
    /// Wall-clock source for persisted timestamps.
    pub clock: Arc<dyn Clock>,
    /// Service bindings the running daemon was booted with. Activating a
    /// generation that differs here requires a restart and is refused.
    pub running_services: ServicesSection,
}

impl ConfigDeps {
    fn now_unix_ms(&self) -> i64 {
        self.clock.now_unix_ms()
    }
}

/// Registers the config lifecycle commands with their canonical types.
pub fn register_handlers(registry: &mut CommandRegistry, deps: ConfigDeps) -> errors::Result<()> {
    registry.register(
        CMD_PROPOSE_CONFIG,
        Arc::new(ProposeHandler { deps: deps.clone() }),
    )?;
    registry.register(
        CMD_MARK_CONFIG_TESTED,
        Arc::new(MarkTestedHandler { deps: deps.clone() }),
    )?;
    registry.register(
        CMD_ACTIVATE_CONFIG,
        Arc::new(ActivateHandler { deps: deps.clone() }),
    )?;
    registry.register(CMD_ROLLBACK_CONFIG, Arc::new(RollbackHandler { deps }))?;
    Ok(())
}

/// `ProposeConfigGeneration` — lands the generation `proposed` and runs
/// schema validation, so a well-formed document arrives `validated` in
/// one command.
struct ProposeHandler {
    deps: ConfigDeps,
}

#[async_trait]
impl CommandHandler for ProposeHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode::<contract::ProposeConfigGeneration>("ProposeConfigGeneration", &payload)?;
        if !raw.digest.is_empty() && raw.digest != document_digest(&raw.document_bytes) {
            return Err(invalid("declared digest does not match the document bytes"));
        }
        let row = generations::propose(
            txn,
            self.deps.ids.as_ref(),
            raw.document_bytes,
            ctx.actor_id,
            self.deps.now_unix_ms(),
        )
        .await?;
        let row = generations::validate(txn, row.generation_id).await?;
        stage_config_event(
            txn,
            self.deps.ids.as_ref(),
            "ConfigProposed",
            contract::ConfigGeneration {
                generation_id: row.generation_id.to_string(),
                digest: row.digest.clone(),
                document: row.document.clone(),
                validation_state: row.validation_state.clone(),
                test_state: row.test_state.clone(),
                created_unix_ms: row.created_at_ms,
            }
            .encode_to_vec(),
            ctx,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: row.generation_id.to_string().into_bytes(),
        })
    }
}

/// `MarkConfigTested` — runs the kernel smoke test (adapter-reference
/// existence) and marks the generation `passed`/`failed`.
struct MarkTestedHandler {
    deps: ConfigDeps,
}

#[async_trait]
impl CommandHandler for MarkTestedHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode::<contract::MarkConfigTested>("MarkConfigTested", &payload)?;
        let generation_id = required_id::<ConfigGenerationId>("generation_id", &raw.generation_id)?;
        check_digest(txn, generation_id, &raw.digest).await?;
        let row = generations::smoke_test(txn, generation_id).await?;
        stage_config_event(
            txn,
            self.deps.ids.as_ref(),
            "ConfigTested",
            contract::ConfigGeneration {
                generation_id: row.generation_id.to_string(),
                digest: row.digest.clone(),
                document: row.document.clone(),
                validation_state: row.validation_state.clone(),
                test_state: row.test_state.clone(),
                created_unix_ms: row.created_at_ms,
            }
            .encode_to_vec(),
            ctx,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: row.test_state.clone().into_bytes(),
        })
    }
}

/// `ActivateConfigGeneration` — CAS-activates a tested generation.
struct ActivateHandler {
    deps: ConfigDeps,
}

#[async_trait]
impl CommandHandler for ActivateHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw =
            decode::<contract::ActivateConfigGeneration>("ActivateConfigGeneration", &payload)?;
        let generation_id = required_id::<ConfigGenerationId>("generation_id", &raw.generation_id)?;
        let active = activate::activate(
            txn,
            generation_id,
            raw.expected_active_revision,
            &self.deps.running_services,
            self.deps.now_unix_ms(),
        )
        .await?;
        stage_config_event(
            txn,
            self.deps.ids.as_ref(),
            "ConfigActivated",
            contract::ConfigGeneration {
                generation_id: generation_id.to_string(),
                digest: String::new(),
                document: Vec::new(),
                validation_state: String::new(),
                test_state: String::new(),
                created_unix_ms: self.deps.now_unix_ms(),
            }
            .encode_to_vec(),
            ctx,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: active.revision.to_string().into_bytes(),
        })
    }
}

/// `RollbackConfigGeneration` — re-activates a prior generation.
struct RollbackHandler {
    deps: ConfigDeps,
}

#[async_trait]
impl CommandHandler for RollbackHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw =
            decode::<contract::RollbackConfigGeneration>("RollbackConfigGeneration", &payload)?;
        let generation_id = required_id::<ConfigGenerationId>("generation_id", &raw.generation_id)?;
        let active = activate::rollback(
            txn,
            generation_id,
            raw.expected_active_revision,
            &self.deps.running_services,
            self.deps.now_unix_ms(),
        )
        .await?;
        stage_config_event(
            txn,
            self.deps.ids.as_ref(),
            "ConfigRolledBack",
            contract::ConfigGeneration {
                generation_id: generation_id.to_string(),
                digest: String::new(),
                document: Vec::new(),
                validation_state: String::new(),
                test_state: String::new(),
                created_unix_ms: self.deps.now_unix_ms(),
            }
            .encode_to_vec(),
            ctx,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: active.revision.to_string().into_bytes(),
        })
    }
}

/// Returns the active generation pointer for `expected_revision`
/// computation by callers.
pub async fn active_revision(txn: &mut dyn KernelTxn) -> errors::Result<u64> {
    Ok(active_state(txn)
        .await?
        .pointer
        .map(|p| p.revision)
        .unwrap_or(0))
}

async fn active_state(txn: &mut dyn KernelTxn) -> errors::Result<ActiveState> {
    activate::active_state(txn).await
}

/// Verifies a caller-supplied digest (when present) matches the row.
async fn check_digest(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
    digest: &str,
) -> errors::Result<()> {
    if digest.is_empty() {
        return Ok(());
    }
    let row = txn
        .config()
        .get_generation(generation_id)
        .await?
        .ok_or_else(|| invalid_code(ErrorCode::NotFound, "config generation not found"))?;
    if row.digest != digest {
        return Err(invalid_code(
            ErrorCode::Conflict,
            "generation digest does not match the stored document",
        ));
    }
    Ok(())
}

/// Stages a catalogued config event on the global config stream.
async fn stage_config_event(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    event_type: &'static str,
    payload: Vec<u8>,
    ctx: &CommandContext,
) -> errors::Result<()> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy
        .minimum(event_type)
        .ok_or_else(|| invalid_code(ErrorCode::Internal, "event type is not catalogued"))?;
    let retention = policy
        .default_retention(event_type)
        .ok_or_else(|| invalid_code(ErrorCode::Internal, "event type is not catalogued"))?;
    let stream_key = EventStreamKey::new(StreamKey::config_global().as_str().to_owned())
        .map_err(|_| invalid_code(ErrorCode::Internal, "stream key is not canonical"))?;
    stage(
        txn,
        DraftEvent {
            event_id: EventId::new(ids),
            stream_key,
            event_type: event_type.to_owned(),
            payload,
            sensitivity,
            retention,
            correlation_id: ctx.correlation_id.clone(),
            causation_id: None,
        },
    )
    .await?;
    Ok(())
}

fn decode<M>(name: &'static str, payload: &[u8]) -> errors::Result<M>
where
    M: prost::Message + Default,
{
    M::decode(payload).map_err(|source| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            format!("{name} payload is not valid contract bytes"),
        )
        .with_source(source)
    })
}

fn required_id<T>(field: &'static str, text: &str) -> errors::Result<T>
where
    T: FromStr,
{
    text.parse().map_err(|_| {
        invalid_code(
            ErrorCode::InvalidArgument,
            format!("{field} is not a valid identifier"),
        )
    })
}

fn invalid(message: impl Into<std::borrow::Cow<'static, str>>) -> KernelError {
    invalid_code(ErrorCode::InvalidArgument, message)
}

fn invalid_code(
    code: ErrorCode,
    message: impl Into<std::borrow::Cow<'static, str>>,
) -> KernelError {
    KernelError::new(code, RetryClass::Never, message)
}
