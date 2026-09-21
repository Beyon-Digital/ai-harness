//! Runtime entity creation: sessions, immutable agent specs, tasks with graph
//! heads, and `Created` runs executed through the command coordinator.
#![forbid(unsafe_code)]

pub mod agent_spec;
pub mod cancel;
pub mod claim;
pub mod create_run;
pub mod decision;
pub mod effect_recovery;
pub mod loop_turn;
pub mod recovery;
pub mod resolved_environment;
pub mod run;
pub mod session;
pub mod state;
pub mod task;
pub mod timers;

use std::str::FromStr;
use std::sync::Arc;

use command_coordinator::handler::CommandRegistry;
use domain::ids::{EventId, EventStreamKey};
use domain::provider::IdProvider;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::outbox::{DraftEvent, stage};
use events::{CatalogClassificationPolicy, ClassificationPolicy, StreamKey};
use kernel_store::KernelTxn;

/// Fully-qualified command type of `CreateSession`.
pub const CMD_CREATE_SESSION: &str = "agentos.spec.v1.CreateSession";
/// Fully-qualified command type of `PutAgentSpecRevision`.
pub const CMD_PUT_AGENT_SPEC_REVISION: &str = "agentos.spec.v1.PutAgentSpecRevision";
/// Fully-qualified command type of `CreateTaskRun`.
pub const CMD_CREATE_TASK_RUN: &str = "agentos.spec.v1.CreateTaskRun";
/// Fully-qualified command type of `ClaimReadyRun`.
pub const CMD_CLAIM_READY_RUN: &str = "agentos.spec.v1.ClaimReadyRun";
/// Fully-qualified command type of `CancelRun`.
pub const CMD_CANCEL_RUN: &str = "agentos.spec.v1.CancelRun";
/// Fully-qualified command type of `ScheduleTimer`.
pub const CMD_SCHEDULE_TIMER: &str = scheduler::CMD_SCHEDULE_TIMER;
/// Fully-qualified command type of `CancelTimer`.
pub const CMD_CANCEL_TIMER: &str = scheduler::CMD_CANCEL_TIMER;
pub use effect_recovery::CMD_RESOLVE_UNKNOWN_EFFECT;

/// Dependencies shared by the runtime command handlers.
#[derive(Clone)]
pub struct RuntimeDeps {
    /// Wall-clock source for persisted timestamps.
    pub clock: Arc<dyn Clock>,
    /// Identifier source for entities and outbox events.
    pub ids: Arc<dyn IdProvider>,
}

impl RuntimeDeps {
    /// Returns the current UTC Unix time in milliseconds.
    pub fn now_unix_ms(&self) -> i64 {
        self.clock.now_unix_ms()
    }
}

/// Registers the entity creation handlers with their canonical command types.
pub fn register_handlers(registry: &mut CommandRegistry, deps: RuntimeDeps) -> errors::Result<()> {
    registry.register(
        CMD_CREATE_SESSION,
        Arc::new(session::CreateSessionHandler::new(deps.clone())),
    )?;
    registry.register(
        CMD_PUT_AGENT_SPEC_REVISION,
        Arc::new(agent_spec::PutAgentSpecRevisionHandler::new(deps.clone())),
    )?;
    registry.register(
        CMD_CREATE_TASK_RUN,
        Arc::new(create_run::CreateTaskRunHandler::new(deps.clone())),
    )?;
    registry.register(
        CMD_CLAIM_READY_RUN,
        Arc::new(claim::ClaimReadyRunHandler::new(deps.clone())),
    )?;
    registry.register(
        CMD_CANCEL_RUN,
        Arc::new(cancel::CancelRunHandler::new(deps.clone())),
    )?;
    registry.register(
        scheduler::CMD_SCHEDULE_TIMER,
        Arc::new(timers::ScheduleTimerHandler::new(deps.clone())),
    )?;
    registry.register(
        scheduler::CMD_CANCEL_TIMER,
        Arc::new(timers::CancelTimerHandler::new(deps.clone())),
    )?;
    registry.register(
        effect_recovery::CMD_RESOLVE_UNKNOWN_EFFECT,
        Arc::new(effect_recovery::ResolveUnknownEffectHandler::new(deps)),
    )?;
    Ok(())
}

/// Stages a catalogued event through the transactional outbox.
///
/// `outbox::stage` allocates the per-stream sequence itself, so the event
/// cannot exist as a built envelope before staging; sensitivity and retention
/// come from the embedded catalog policy, which enforces the classification
/// floor for the catalogue entry.
pub(crate) async fn stage_catalogued(
    txn: &mut dyn KernelTxn,
    event_id: EventId,
    event_type: &str,
    stream: StreamKey,
    payload: Vec<u8>,
    correlation_id: Option<String>,
    causation_id: Option<EventId>,
) -> errors::Result<u64> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy
        .minimum(event_type)
        .ok_or_else(|| uncatalogued(event_type))?;
    let retention = policy
        .default_retention(event_type)
        .ok_or_else(|| uncatalogued(event_type))?;
    let stream_key = EventStreamKey::new(stream.as_str().to_owned()).map_err(|_| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "catalogued stream key is not canonical",
        )
    })?;
    stage(
        txn,
        DraftEvent {
            event_id,
            stream_key,
            event_type: event_type.to_owned(),
            payload,
            sensitivity,
            retention,
            correlation_id,
            causation_id,
        },
    )
    .await
}

/// Decodes a prost contract message without echoing the payload bytes.
pub(crate) fn decode_contract<M>(name: &'static str, payload: &[u8]) -> errors::Result<M>
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

/// Parses a required identifier field without echoing its value.
pub(crate) fn required_id<T>(field: &'static str, text: &str) -> errors::Result<T>
where
    T: FromStr,
{
    text.parse()
        .map_err(|_| invalid_field(field, "is not a valid identifier"))
}

/// Parses an optional identifier field; an empty string means absent.
pub(crate) fn optional_id<T>(field: &'static str, text: &str) -> errors::Result<Option<T>>
where
    T: FromStr,
{
    if text.is_empty() {
        return Ok(None);
    }
    required_id(field, text).map(Some)
}

fn invalid_field(field: &'static str, detail: &'static str) -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        format!("{field} {detail}"),
    )
}

fn uncatalogued(event_type: &str) -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        format!("event type {event_type} has no catalog entry"),
    )
}

#[cfg(test)]
mod tests {
    use errors::codes::{ErrorCode, RetryClass};

    use super::{decode_contract, optional_id, required_id};

    #[test]
    fn malformed_payloads_are_invalid_arguments_without_echoing_bytes() {
        const MARKER: &[u8] = b"do-not-echo-payload-marker-7c1f";
        let error =
            decode_contract::<domain::generated::contract::CreateSession>("CreateSession", MARKER)
                .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(!error.message().contains("do-not-echo"));
    }

    #[test]
    fn malformed_identifiers_are_invalid_arguments_without_echoing_values() {
        let error = required_id::<domain::ids::RunId>("run_id", "do-not-echo-run-id-marker-7c1f")
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(!error.message().contains("do-not-echo"));
    }

    #[test]
    fn empty_optional_identifiers_are_absent() {
        let absent: Option<domain::ids::RunId> = optional_id("run_id", "").unwrap();
        assert!(absent.is_none());
    }
}
