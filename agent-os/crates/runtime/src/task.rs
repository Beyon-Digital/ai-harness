//! Task creation with its graph head in the caller's transaction.

use kernel_store::KernelTxn;
use kernel_store::models::NewTask;

/// Inserts the task when it does not exist yet and ensures its
/// `run_graph_heads` row exists, both inside `txn` (D16).
///
/// The existence check makes task creation idempotent when several runs are
/// created against the same task; the head insert is itself idempotent.
pub async fn ensure_task(txn: &mut dyn KernelTxn, task: NewTask) -> errors::Result<()> {
    let task_id = task.task_id;
    if txn.tasks().get(task_id).await?.is_none() {
        txn.tasks().insert(task).await?;
    }
    txn.graph().ensure_head(task_id).await?;
    Ok(())
}
