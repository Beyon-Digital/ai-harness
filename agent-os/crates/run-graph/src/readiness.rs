//! Dependency readiness: condition evaluation and target satisfaction.
//!
//! A `Ready` run may be claimed only when every dependency edge into it is
//! satisfied by the persisted state of its source run. Evaluation happens
//! inside the caller's transaction so the decision and the claim commit
//! atomically (R4.1, R4.2).

use domain::ids::RunId;
use domain::resource::DependencyCondition;
use domain::run::RunState;
use kernel_store::KernelTxn;

use crate::repository::load_run;

/// Returns true when `source_state` satisfies `condition`.
///
/// `Unspecified` is never satisfied; the graph service rejects such edges
/// before they are persisted, so the arm is defensive.
pub const fn condition_met(condition: DependencyCondition, source_state: RunState) -> bool {
    match condition {
        DependencyCondition::CompletedSuccessfully => {
            matches!(source_state, RunState::Completed)
        }
        DependencyCondition::AnyTerminal => source_state.is_terminal(),
        DependencyCondition::CompletedOrCancelled => {
            matches!(source_state, RunState::Completed | RunState::Cancelled)
        }
        DependencyCondition::Unspecified => false,
    }
}

/// Returns true when every dependency targeting `target` is satisfied by its
/// source run's persisted state. Runs without dependencies are satisfied.
///
/// Sources always exist when the edge exists (the graph service validates both
/// endpoints before inserting); a missing source is `NotFound`, never a
/// guessed state.
pub async fn dependencies_satisfied(
    txn: &mut dyn KernelTxn,
    target: RunId,
) -> errors::Result<bool> {
    let target_row = load_run(txn, target).await?;
    let dependencies = txn.graph().list_dependencies(target_row.task_id).await?;
    for dependency in dependencies
        .iter()
        .filter(|row| row.target_run_id == target)
    {
        let source = load_run(txn, dependency.source_run_id).await?;
        if !condition_met(dependency.dependency_condition, source.state) {
            return Ok(false);
        }
    }
    Ok(true)
}
