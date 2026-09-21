//! Generation activation (CFG-002): CAS the active pointer, restricted
//! to live-safe changes.
//!
//! Generation-global service bindings (`services:`) are frozen for the
//! daemon instance's lifetime — a generation that would rebind them
//! cannot activate live: activation returns `DAEMON_RESTART_REQUIRED`
//! and the pointer does not move. Run-scoped changes activate freely.
//! Rollback is the same path aimed at a prior known-good generation.
#![forbid(unsafe_code)]

use domain::ids::ConfigGenerationId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{ActiveConfigGenerationRow, ConfigGenerationRow};
use kernel_store::txn::KernelTxn;

use crate::generations::{testing, validation};
use crate::model;
use crate::schema::{ConfigDocument, ServicesSection};

fn activate_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// The stable error detail a daemon restart requirement carries.
pub const DAEMON_RESTART_REQUIRED: &str = "DAEMON_RESTART_REQUIRED";

/// Current activation state: pointer + the generation's services block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveState {
    /// Pointer row (`None` when nothing has ever activated).
    pub pointer: Option<ActiveConfigGenerationRow>,
    /// Generation the pointer references (`None` with the pointer).
    pub generation: Option<ConfigGenerationRow>,
}

/// Read the active pointer and its generation.
pub async fn active_state(txn: &mut dyn KernelTxn) -> errors::Result<ActiveState> {
    let pointer = txn.config().get_active().await?;
    let generation = match &pointer {
        Some(p) => txn.config().get_generation(p.generation_id).await?,
        None => None,
    };
    Ok(ActiveState {
        pointer,
        generation,
    })
}

/// Activate `generation_id` at `expected_revision` (CAS on the pointer's
/// revision; 0 when no pointer exists).
///
/// Eligibility: `validation_state == validated` AND `test_state ==
/// passed` — an untested or failed generation cannot activate, and a
/// rejected document is refused outright. A generation whose
/// `services:` differ from the running daemon's current bindings fails
/// `DAEMON_RESTART_REQUIRED` without moving the pointer.
pub async fn activate(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
    expected_revision: u64,
    running_services: &ServicesSection,
    now_ms: i64,
) -> errors::Result<ActiveConfigGenerationRow> {
    let row = txn
        .config()
        .get_generation(generation_id)
        .await?
        .ok_or_else(|| activate_error(ErrorCode::NotFound, "config generation not found"))?;
    if row.validation_state != validation::VALIDATED || row.test_state != testing::PASSED {
        return Err(activate_error(
            ErrorCode::FailedPrecondition,
            format!(
                "generation is not eligible (validation={}, test={})",
                row.validation_state, row.test_state
            ),
        ));
    }
    let doc: ConfigDocument = model::parse_document(
        std::str::from_utf8(&row.document)
            .map_err(|_| activate_error(ErrorCode::InvalidArgument, "document not utf-8"))?,
    )?;
    if model::generation_services(&doc) != *running_services {
        return Err(activate_error(
            ErrorCode::FailedPrecondition,
            format!(
                "{DAEMON_RESTART_REQUIRED}: generation-global service bindings differ from \
                 the running daemon's; restart to adopt them"
            ),
        ));
    }
    let moved = txn
        .config()
        .cas_active(expected_revision, generation_id, now_ms)
        .await?;
    if !moved {
        return Err(activate_error(
            ErrorCode::Conflict,
            "active generation revision moved during activation",
        ));
    }
    txn.config()
        .get_active()
        .await?
        .ok_or_else(|| activate_error(ErrorCode::Internal, "activated pointer not visible"))
}

/// Reactivate a prior known-good generation — identical checks (a
/// `RollbackConfigGeneration` path); only generations that previously
/// passed validation+testing are eligible.
pub async fn rollback(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
    expected_revision: u64,
    running_services: &ServicesSection,
    now_ms: i64,
) -> errors::Result<ActiveConfigGenerationRow> {
    activate(
        txn,
        generation_id,
        expected_revision,
        running_services,
        now_ms,
    )
    .await
}
