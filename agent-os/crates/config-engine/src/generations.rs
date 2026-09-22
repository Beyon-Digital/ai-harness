//! Immutable config generations (CFG-002): propose, validate, mark
//! tested — the pipeline before activation.
//!
//! A generation is `proposed -> validated|rejected` and independently
//! `untested -> passed|failed`. Only `validated + passed` generations
//! are eligible for activation; every transition is a forward-only
//! state machine enforced here plus the CHECK domain in the schema.
#![forbid(unsafe_code)]

use domain::ids::{ActorId, ConfigGenerationId};
use domain::provider::IdProvider;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{ConfigGenerationRow, NewConfigGeneration};
use kernel_store::txn::KernelTxn;

use crate::model;

fn gen_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Persisted `validation_state` literals.
pub mod validation {
    /// Parsed but not yet validated.
    pub const PROPOSED: &str = "proposed";
    /// Schema + references validated.
    pub const VALIDATED: &str = "validated";
    /// Validation failed.
    pub const REJECTED: &str = "rejected";
}

/// Persisted `test_state` literals.
pub mod testing {
    /// Not yet smoke-tested.
    pub const UNTESTED: &str = "untested";
    /// Smoke checks passed.
    pub const PASSED: &str = "passed";
    /// Smoke checks failed.
    pub const FAILED: &str = "failed";
}

/// Propose a document as a new immutable generation. The row lands
/// `proposed`/`untested`; `validate` and `mark_tested` advance it.
/// Identical bytes reproduce the same digest — proposing a duplicate is
/// a `Conflict` (the registry never holds two rows for one document).
pub async fn propose(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    document: Vec<u8>,
    actor: ActorId,
    now_ms: i64,
) -> errors::Result<ConfigGenerationRow> {
    let digest = model::document_digest(&document);
    let generation_id = ConfigGenerationId::new(ids);
    txn.config()
        .insert_generation(NewConfigGeneration {
            generation_id,
            digest: digest.clone(),
            document,
            validation_state: validation::PROPOSED.to_owned(),
            test_state: testing::UNTESTED.to_owned(),
            created_by_actor_id: actor,
            created_at_ms: now_ms,
        })
        .await?;
    txn.config()
        .get_generation(generation_id)
        .await?
        .ok_or_else(|| gen_error(ErrorCode::Internal, "inserted generation not visible"))
}

/// Schema validation. A parse failure marks the row `rejected` and
/// returns the parse error; success marks it `validated`. Neither moves
/// the active pointer.
pub async fn validate(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
) -> errors::Result<ConfigGenerationRow> {
    let row = get(txn, generation_id).await?;
    let valid = std::str::from_utf8(&row.document)
        .map_err(|e| gen_error(ErrorCode::InvalidArgument, format!("not utf-8: {e}")))
        .and_then(model::parse_document);
    match valid {
        Ok(_) => {
            txn.config()
                .set_generation_states(generation_id, Some(validation::VALIDATED), None)
                .await?;
            get(txn, generation_id).await
        }
        Err(err) => {
            txn.config()
                .set_generation_states(generation_id, Some(validation::REJECTED), None)
                .await?;
            Err(err)
        }
    }
}

/// Smoke checks beyond schema: every adapter referenced by every profile
/// binding must currently be registered (existence check; version pin is
/// resolved at run-bind time). On failure the row is marked `failed` and
/// the error returned — the active pointer never moved.
pub async fn smoke_test(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
) -> errors::Result<ConfigGenerationRow> {
    let row = get(txn, generation_id).await?;
    if row.validation_state != validation::VALIDATED {
        return Err(gen_error(
            ErrorCode::FailedPrecondition,
            "generation must be validated before smoke testing",
        ));
    }
    let doc = model::parse_document(std::str::from_utf8(&row.document).map_err(|_| {
        gen_error(
            ErrorCode::InvalidArgument,
            "generation document is not utf-8",
        )
    })?)?;
    let outcome = validate_adapter_refs(txn, &doc).await;
    match outcome {
        Ok(()) => {
            txn.config()
                .set_generation_states(generation_id, None, Some(testing::PASSED))
                .await?;
            get(txn, generation_id).await
        }
        Err(err) => {
            txn.config()
                .set_generation_states(generation_id, None, Some(testing::FAILED))
                .await?;
            Err(err)
        }
    }
}

/// In-tree adapters always available — they aren't registry entries.
pub const BUILTIN_ADAPTERS: &[&str] = &[
    "local-process-t0",
    "local-workspace",
    "local-artifacts",
    "fixture-loop",
];

/// Every `port: "name@version"` binding resolves: builtin names are the
/// in-tree adapters shipped with the daemon; any other name must be a
/// registered `adapter_id`.
async fn validate_adapter_refs(
    txn: &mut dyn KernelTxn,
    doc: &model::ConfigDocument,
) -> errors::Result<()> {
    let registered = txn.adapters().list_registrations().await?;
    for profile_name in doc.profiles.keys() {
        let resolved = crate::profile::resolve_profile(doc, profile_name)?;
        for (slot, binding) in &resolved.bindings {
            let name = binding.split('@').next().unwrap_or(binding);
            let exists = BUILTIN_ADAPTERS.contains(&name)
                || registered
                    .iter()
                    .any(|row| row.adapter_id.to_string() == name);
            if !exists {
                return Err(gen_error(
                    ErrorCode::FailedPrecondition,
                    format!("profile '{profile_name}' binds missing adapter '{binding}' on {slot}"),
                ));
            }
        }
    }
    Ok(())
}

async fn get(
    txn: &mut dyn KernelTxn,
    generation_id: ConfigGenerationId,
) -> errors::Result<ConfigGenerationRow> {
    txn.config()
        .get_generation(generation_id)
        .await?
        .ok_or_else(|| gen_error(ErrorCode::NotFound, "config generation not found"))
}
