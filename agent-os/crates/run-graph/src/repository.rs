//! Read helpers over the kernel-store ports for run-graph services.
//!
//! Ancestry is derived exclusively from `runs.parent_run_id` through
//! [`RunRepo::list_by_task`](kernel_store::repositories::RunRepo::list_by_task);
//! the graph stores dependency edges only and never a second ancestry table
//! (R3.6). Cycle logic stays in `GraphRepo::insert_dependency`, which checks
//! reachability inside the same immediate transaction as the insert.

use std::collections::{HashMap, VecDeque};

use domain::ids::{RunId, TaskId};
use domain::run::RunState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{RunDependencyRow, RunRow};

/// Loads a run or fails with `NotFound` when the identifier is unknown.
pub async fn load_run(txn: &mut dyn KernelTxn, run_id: RunId) -> errors::Result<RunRow> {
    txn.runs().get(run_id).await?.ok_or_else(|| {
        KernelError::new(ErrorCode::NotFound, RetryClass::Never, "run does not exist")
    })
}

/// Returns true when the run carries a claim whose expiry is still in the
/// future at `now_ms`; expired claims are reclaimable.
pub fn has_live_claim(run: &RunRow, now_ms: i64) -> bool {
    run.claim_expires_ms
        .is_some_and(|expires_unix_ms| expires_unix_ms > now_ms)
}

/// Returns true when new dependency edges may target the run: only `Created`
/// runs or `Ready` runs without a live claim are mutable (R3.1).
pub fn is_mutable_target(run: &RunRow, now_ms: i64) -> bool {
    run.state == RunState::Created || (run.state == RunState::Ready && !has_live_claim(run, now_ms))
}

/// Finds the already-persisted edge for `(source, target)` within a task.
pub async fn find_dependency(
    txn: &mut dyn KernelTxn,
    task: TaskId,
    source: RunId,
    target: RunId,
) -> errors::Result<Option<RunDependencyRow>> {
    Ok(txn
        .graph()
        .list_dependencies(task)
        .await?
        .into_iter()
        .find(|row| row.source_run_id == source && row.target_run_id == target))
}

/// Returns every run descended from `root` through `runs.parent_run_id`,
/// walked in memory within the root's task and ordered by breadth-first
/// traversal of the run-id-ordered task listing. The root itself is not
/// included.
pub async fn descendants(txn: &mut dyn KernelTxn, root: RunId) -> errors::Result<Vec<RunId>> {
    let root_row = load_run(txn, root).await?;
    let runs = txn.runs().list_by_task(root_row.task_id).await?;

    let mut children: HashMap<RunId, Vec<RunId>> = HashMap::new();
    for run in &runs {
        if let Some(parent) = run.parent_run_id {
            children.entry(parent).or_default().push(run.run_id);
        }
    }

    let mut ordered = Vec::new();
    let mut frontier = VecDeque::from([root]);
    while let Some(current) = frontier.pop_front() {
        for child in children.get(&current).into_iter().flatten() {
            ordered.push(*child);
            frontier.push_back(*child);
        }
    }
    Ok(ordered)
}
