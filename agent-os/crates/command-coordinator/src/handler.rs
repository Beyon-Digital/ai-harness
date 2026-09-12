//! Typed handler dispatch: the context handlers receive, the success outcome
//! vocabulary, the handler trait, and the command-type registry (R1.1, R1.4,
//! R3.1, R3.2, R3.4).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use domain::ids::{ActorId, CommandId, DelegationChainId, DeviceId, PrincipalId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// Context passed to a handler: the envelope's routing fields, never the
/// payload digest or the transaction handle owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandContext {
    /// Identity of this submission.
    pub command_id: CommandId,
    /// Principal on whose behalf the command runs.
    pub principal_id: PrincipalId,
    /// Actor that issued the command.
    pub actor_id: ActorId,
    /// Optional device that issued the command.
    pub device_id: Option<DeviceId>,
    /// Optional delegation chain authorizing the actor.
    pub delegation_chain_id: Option<DelegationChainId>,
    /// Optional correlation identifier for observability.
    pub correlation_id: Option<String>,
    /// Optional identifier of the command that caused this one.
    pub causation_id: Option<String>,
}

/// Persisted outcome vocabulary. Only success is recorded today, so replay of
/// a failure is impossible by construction (design ruling 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeCode {
    /// The command's mutations and events were applied and committed.
    Ok,
}

impl OutcomeCode {
    /// Returns the stable persisted token for this code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
        }
    }

    /// Decodes a persisted token, failing closed on values this version does
    /// not understand.
    pub fn from_stored(value: &str) -> errors::Result<Self> {
        match value {
            "ok" => Ok(Self::Ok),
            _ => Err(KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "stored idempotency outcome_code is not recognized",
            )),
        }
    }
}

/// Result of a handler run: outcome code plus callback payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    /// Persisted outcome code.
    pub code: OutcomeCode,
    /// Serialized outcome payload; never logged.
    pub payload: Vec<u8>,
}

/// Command implementation bound to one command type.
///
/// Handlers see only the context and the write transaction; they stage events
/// through `events::outbox::stage` and never see the store.
#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    /// Applies the command's mutations inside `txn` and reports its outcome.
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn kernel_store::KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome>;
}

/// Command-type to handler map; registration is once-only.
#[derive(Default)]
pub struct CommandRegistry {
    handlers: HashMap<String, Arc<dyn CommandHandler>>,
}

impl CommandRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `handler` for `command_type`.
    ///
    /// Returns `Conflict` when the command type is already registered, leaving
    /// the existing handler in place.
    pub fn register(
        &mut self,
        command_type: impl Into<String>,
        handler: Arc<dyn CommandHandler>,
    ) -> errors::Result<()> {
        let command_type = command_type.into();
        if command_type.trim().is_empty() {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "command_type must not be empty",
            ));
        }
        match self.handlers.entry(command_type) {
            Entry::Occupied(_) => Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "command_type is already registered",
            )),
            Entry::Vacant(entry) => {
                entry.insert(handler);
                Ok(())
            }
        }
    }

    /// Returns the handler registered for `command_type`, if any.
    pub fn get(&self, command_type: &str) -> Option<Arc<dyn CommandHandler>> {
        self.handlers.get(command_type).cloned()
    }

    /// Returns the number of registered command types.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns whether no command types are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use errors::codes::{ErrorCode, RetryClass};

    use super::{CommandContext, CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode};

    struct NoopHandler;

    #[async_trait::async_trait]
    impl CommandHandler for NoopHandler {
        async fn handle(
            &self,
            _ctx: &CommandContext,
            _txn: &mut dyn kernel_store::KernelTxn,
            _payload: Vec<u8>,
        ) -> errors::Result<CommandOutcome> {
            Ok(CommandOutcome {
                code: OutcomeCode::Ok,
                payload: Vec::new(),
            })
        }
    }

    #[test]
    fn outcome_codes_round_trip_and_fail_closed() {
        assert_eq!(OutcomeCode::Ok.as_str(), "ok");
        assert_eq!(OutcomeCode::from_stored("ok").unwrap(), OutcomeCode::Ok);
        let error = match OutcomeCode::from_stored("failed") {
            Ok(_) => panic!("unknown stored code was admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.retry_class(), RetryClass::Never);
    }

    #[test]
    fn duplicate_registration_is_a_conflict() {
        let mut registry = CommandRegistry::new();
        assert!(registry.is_empty());
        registry
            .register("session.create", Arc::new(NoopHandler))
            .expect("first registration succeeds");
        let error = match registry.register("session.create", Arc::new(NoopHandler)) {
            Ok(()) => panic!("duplicate registration was admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("session.create").is_some());
        assert!(registry.get("session.delete").is_none());
    }

    #[test]
    fn empty_command_types_are_rejected() {
        let mut registry = CommandRegistry::new();
        let error = match registry.register("  ", Arc::new(NoopHandler)) {
            Ok(()) => panic!("empty command type was admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert!(registry.is_empty());
    }
}
