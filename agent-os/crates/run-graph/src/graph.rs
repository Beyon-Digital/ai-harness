//! Dependency edge mutation: same-task scope, mutable targets, deterministic
//! duplicates, cycle rejection, and the `DependencyAdded` outbox event.
//!
//! Cycle detection is not reimplemented here: `GraphRepo::insert_dependency`
//! checks reachability and advances the task's graph head inside one immediate
//! transaction (R3.3). This service adds the domain validations, the duplicate
//! outcome, and stages the catalogued event through the transactional outbox in
//! the caller's transaction (R3.1, R3.2, R3.4).

use domain::generated::contract;
use domain::ids::{DependencyId, EventId, EventStreamKey, RunId, TaskId};
use domain::provider::SystemIdProvider;
use domain::resource::DependencyCondition;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::outbox::{DraftEvent, stage};
use events::{CatalogClassificationPolicy, ClassificationPolicy, StreamKey};
use kernel_store::KernelTxn;
use kernel_store::models::NewRunDependency;
use prost::Message;

use crate::repository::{find_dependency, is_mutable_target, load_run};

/// Catalogued event type staged when an edge commits.
const DEPENDENCY_ADDED: &str = "DependencyAdded";

/// Adds `source -> target` to their task's dependency graph.
///
/// The source and target must be persisted runs of the same task and the
/// target must be mutable: `Created`, or `Ready` without a live claim (R3.1).
/// `expected_revision` must equal the task's persisted graph head, or the call
/// conflicts without mutation. An identical edge is an idempotent no-op; the
/// same endpoints with another condition conflict. Cycle-forming edges are
/// rejected by the store's reachability check inside the immediate transaction
/// (R3.2). On success the head advances and `DependencyAdded` is staged on the
/// `task/{task_id}` stream in the same transaction (R3.3).
pub async fn add_dependency(
    txn: &mut dyn KernelTxn,
    source: RunId,
    target: RunId,
    condition: DependencyCondition,
    expected_revision: u64,
    now_ms: i64,
) -> errors::Result<()> {
    if condition == DependencyCondition::Unspecified {
        return Err(failed_precondition("dependency condition is unspecified"));
    }

    let source_row = load_run(txn, source).await?;
    let target_row = load_run(txn, target).await?;
    if source_row.task_id != target_row.task_id {
        return Err(failed_precondition(
            "dependency endpoints belong to different tasks",
        ));
    }
    if !is_mutable_target(&target_row, now_ms) {
        return Err(failed_precondition(
            "dependency target is not Created or unclaimed Ready",
        ));
    }
    let task_id = target_row.task_id;

    let head = txn.graph().get_head(task_id).await?.ok_or_else(|| {
        KernelError::new(
            ErrorCode::NotFound,
            RetryClass::Never,
            "graph head missing for task",
        )
    })?;
    if head.graph_revision != expected_revision {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "graph revision does not match",
        ));
    }

    if let Some(existing) = find_dependency(txn, task_id, source, target).await? {
        if existing.dependency_condition == condition {
            return Ok(());
        }
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "duplicate run dependency has a different condition",
        ));
    }

    let dependency_id = DependencyId::new(&SystemIdProvider);
    txn.graph()
        .insert_dependency(
            NewRunDependency {
                dependency_id,
                task_id,
                source_run_id: source,
                target_run_id: target,
                dependency_condition: condition,
                created_at_ms: now_ms,
            },
            expected_revision,
        )
        .await?;

    let payload = contract::RunDependency {
        dependency_id: dependency_id.to_string(),
        task_id: task_id.to_string(),
        source_run_id: source.to_string(),
        target_run_id: target.to_string(),
        condition: condition.to_wire(),
        created_graph_revision: expected_revision,
    }
    .encode_to_vec();
    stage_event(txn, task_id, payload).await
}

/// Stages the catalogued `DependencyAdded` payload on the task stream.
async fn stage_event(
    txn: &mut dyn KernelTxn,
    task_id: TaskId,
    payload: Vec<u8>,
) -> errors::Result<()> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy.minimum(DEPENDENCY_ADDED).ok_or_else(uncatalogued)?;
    let retention = policy
        .default_retention(DEPENDENCY_ADDED)
        .ok_or_else(uncatalogued)?;
    let stream_key = EventStreamKey::new(StreamKey::task(task_id).as_str()).map_err(|_| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "catalogued stream key is not canonical",
        )
    })?;
    stage(
        txn,
        DraftEvent {
            event_id: EventId::new(&SystemIdProvider),
            stream_key,
            event_type: DEPENDENCY_ADDED.to_owned(),
            payload,
            sensitivity,
            retention,
            correlation_id: None,
            causation_id: None,
        },
    )
    .await?;
    Ok(())
}

fn failed_precondition(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, detail)
}

fn uncatalogued() -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        "event type DependencyAdded has no catalog entry",
    )
}
