//! Classification, redaction, and tracing helpers for the daemon.
#![forbid(unsafe_code)]

pub mod classification;
pub use classification::{Classification, Classified, Redacted, Secret, is_visible};

/// Kernel correlation identifiers and data classification attached to every
/// signal.
#[derive(Clone, Debug, Default)]
pub struct KernelFields {
    pub correlation_id: Option<String>,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub effect_id: Option<String>,
    /// Sensitivity of the signal; defaults to [`Classification::Internal`].
    pub classification: Classification,
}

/// Installs a `tracing_subscriber` sink, JSON-encoded when `json` is true.
pub fn init_tracing(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let builder = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    if json {
        builder.json().try_init().map_err(std::io::Error::other)?;
    } else {
        builder.try_init().map_err(std::io::Error::other)?;
    }
    Ok(())
}

/// Emits one structured audit record for a completed sensitive operation.
///
/// Only the provided fields are recorded: the operation kind, the actor, the
/// run when the operation is run-scoped, the target identifier, and the
/// terminal outcome. Callers must never pass secret values or raw payload
/// bytes; the function itself redacts nothing, so passing a value is a caller
/// defect. The record is emitted as a single `audit.record` tracing event, so
/// production wiring reaches it through the installed subscriber.
pub fn audit_record(kind: &str, actor_id: &str, run_id: Option<&str>, target: &str, outcome: &str) {
    match run_id {
        Some(run_id) => tracing::info!(
            kind = kind,
            actor_id = actor_id,
            run_id = run_id,
            target = target,
            outcome = outcome,
            "audit.record"
        ),
        None => tracing::info!(
            kind = kind,
            actor_id = actor_id,
            run_id = tracing::field::Empty,
            target = target,
            outcome = outcome,
            "audit.record"
        ),
    }
}

/// Builds a span carrying the correlation, run, task, and effect ids when
/// present, plus the signal's data classification.
pub fn kernel_span(fields: &KernelFields) -> tracing::Span {
    let span = tracing::info_span!(
        "kernel.span",
        correlation_id = tracing::field::Empty,
        run_id = tracing::field::Empty,
        task_id = tracing::field::Empty,
        effect_id = tracing::field::Empty,
        classification = tracing::field::Empty,
    );
    if let Some(correlation_id) = fields.correlation_id.as_deref() {
        span.record("correlation_id", correlation_id);
    }
    if let Some(run_id) = fields.run_id.as_deref() {
        span.record("run_id", run_id);
    }
    if let Some(task_id) = fields.task_id.as_deref() {
        span.record("task_id", task_id);
    }
    if let Some(effect_id) = fields.effect_id.as_deref() {
        span.record("effect_id", effect_id);
    }
    span.record(
        "classification",
        tracing::field::debug(fields.classification),
    );
    span
}
