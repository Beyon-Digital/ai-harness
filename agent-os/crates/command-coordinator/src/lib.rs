//! Linearization, idempotency, and atomic command orchestration (R1, R2, R4).
//!
//! [`CommandCoordinator::execute`] is the single mutation path for commands:
//! registry lookup, envelope validation, a fenced write transaction, replay
//! lookup, handler dispatch, fault points around commit, and success-only
//! idempotency recording. Handlers stage events through `events::outbox`;
//! this crate never issues SQL and never opens a transaction outside the
//! injected [`KernelStore`].
#![forbid(unsafe_code)]

pub mod envelope;
pub mod handler;

use std::sync::Arc;

use domain::faults::FaultInjector;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::NewIdempotencyRecord;
use kernel_store::{KernelStore, TxContext};

use crate::envelope::CommandEnvelope;
use crate::handler::{CommandContext, CommandOutcome, CommandRegistry, OutcomeCode};

/// Fault point consulted after the handler stages work and before commit.
pub const BEFORE_COMMIT: &str = "command.before_commit";

/// Fault point consulted after commit and before the outcome is acknowledged.
pub const AFTER_COMMIT: &str = "command.after_commit";

/// Supplies the daemon fencing epoch used to open write transactions.
pub trait FenceProvider: Send + Sync {
    /// Returns the epoch the coordinator currently holds.
    fn epoch(&self) -> u64;
}

/// [`FenceProvider`] pinned to one epoch for composition and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedFence(pub u64);

impl FenceProvider for FixedFence {
    fn epoch(&self) -> u64 {
        self.0
    }
}

/// Executes registered commands against the kernel store.
pub struct CommandCoordinator {
    store: Arc<dyn KernelStore>,
    registry: Arc<CommandRegistry>,
    fence: Arc<dyn FenceProvider>,
    clock: Arc<dyn Clock>,
    faults: Arc<dyn FaultInjector>,
}

impl CommandCoordinator {
    /// Wires the coordinator to its persistence, dispatch, fencing, clock,
    /// and fault seams.
    pub fn new(
        store: Arc<dyn KernelStore>,
        registry: Arc<CommandRegistry>,
        fence: Arc<dyn FenceProvider>,
        clock: Arc<dyn Clock>,
        faults: Arc<dyn FaultInjector>,
    ) -> Self {
        Self {
            store,
            registry,
            fence,
            clock,
            faults,
        }
    }

    /// Runs the ten-step execute algorithm and returns the outcome.
    ///
    /// Fresh commands commit their mutations, staged outbox events, and an
    /// `ok` idempotency record atomically. An identical replay returns the
    /// stored outcome without touching state; a differing digest is a
    /// `Conflict`. The `command.before_commit` fault point rolls back, and
    /// `command.after_commit` reports `Unavailable` after a durable commit.
    pub async fn execute(&self, envelope: CommandEnvelope) -> errors::Result<CommandOutcome> {
        let handler = self.registry.get(&envelope.command_type).ok_or_else(|| {
            KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "command_type is not registered in the coordinator",
            )
        })?;

        envelope.validate(self.clock.now_unix_ms())?;

        let context = CommandContext {
            command_id: envelope.command_id,
            principal_id: envelope.principal_id,
            actor_id: envelope.actor_id,
            device_id: envelope.device_id,
            delegation_chain_id: envelope.delegation_chain_id,
            correlation_id: envelope.correlation_id.clone(),
            causation_id: envelope.causation_id.clone(),
        };
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch: self.fence.epoch(),
                principal_id: envelope.principal_id,
                command_id: envelope.command_id,
                correlation_id: envelope.correlation_id.clone(),
            })
            .await?;

        if let Some(stored) = txn
            .idempotency()
            .lookup(envelope.principal_id, &envelope.idempotency_key)
            .await?
        {
            if stored.request_digest != envelope.request_digest.to_string() {
                drop(txn);
                return Err(KernelError::new(
                    ErrorCode::Conflict,
                    RetryClass::Never,
                    "request_digest differs from the record stored for idempotency_key",
                ));
            }
            let outcome = CommandOutcome {
                code: OutcomeCode::from_stored(&stored.outcome_code)?,
                payload: stored.outcome_payload,
            };
            drop(txn);
            return Ok(outcome);
        }

        let outcome = handler
            .handle(&context, &mut *txn, envelope.payload)
            .await?;

        if self.faults.inject(BEFORE_COMMIT) {
            drop(txn);
            return Err(KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "command.before_commit fault point aborted the transaction",
            ));
        }

        txn.idempotency()
            .insert(NewIdempotencyRecord {
                principal_id: envelope.principal_id,
                idempotency_key: envelope.idempotency_key,
                request_digest: envelope.request_digest.to_string(),
                command_id: envelope.command_id,
                outcome_code: outcome.code.as_str().to_owned(),
                outcome_payload: outcome.payload.clone(),
                created_at_ms: self.clock.now_unix_ms(),
            })
            .await?;

        txn.commit().await?;

        if self.faults.inject(AFTER_COMMIT) {
            return Err(KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "command.after_commit fault point fired after commit",
            ));
        }

        Ok(outcome)
    }
}
