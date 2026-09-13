//! Startup recovery disposition classification and reconciliation hook.
//!
//! Thin call-through over [`runtime::recovery::reconstruct`]; the composition
//! root wires [`run_startup_recovery`] into the daemon lifecycle after the
//! daemon fence is acquired (INT-001). Until then the hook is not reachable
//! from `main`, so the module allows dead code (matching `lock` and the
//! unwired workers).
#![allow(dead_code)]

use std::sync::Arc;

use kernel_store::KernelStore;
use runtime::recovery::{Clock, RecoveryReport, reconstruct};

/// Reconstructs startup state and persists every changed recovery
/// disposition, aborting on an unmapped combination.
pub async fn run_startup_recovery(
    store: &dyn KernelStore,
    clock: Arc<dyn Clock>,
) -> errors::Result<RecoveryReport> {
    reconstruct(store, clock).await
}
