//! Daemon-instance lifecycle: claim the durable fence and expose the handle.

use domain::ids::DaemonInstanceId;
use domain::provider::IdProvider;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::{DaemonFence, KernelStore};

/// A claimed daemon instance and the durable fence it owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonHandle {
    /// Identifier minted for this daemon instance.
    pub instance_id: DaemonInstanceId,
    /// Persisted fence claimed by the instance.
    pub fence: DaemonFence,
}

/// Claims the daemon fence for a fresh instance drawn from `ids`.
///
/// The instance identifier is caller-supplied through the [`IdProvider`], and
/// the returned handle carries the persisted [`DaemonFence`]. `lease_ms` is the
/// requested lease duration; it is validated as a positive lifetime, while the
/// durable lease written to `daemon_fence` is the store's normative
/// `daemon.fence_lease_ms`, because the persisted epoch row is the single
/// authority (D8).
pub async fn acquire(
    store: &dyn KernelStore,
    ids: &dyn IdProvider,
    lease_ms: i64,
) -> errors::Result<DaemonHandle> {
    if lease_ms <= 0 {
        return Err(KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "daemon fence lease must be a positive number of milliseconds",
        ));
    }
    let instance_id = DaemonInstanceId::new(ids);
    let fence = store.acquire_daemon_fence(instance_id).await?;
    Ok(DaemonHandle { instance_id, fence })
}
