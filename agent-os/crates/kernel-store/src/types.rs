//! Transaction context and daemon fencing value types.

use domain::ids::{CommandId, DaemonInstanceId, PrincipalId};

/// Context carried by every write transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxContext {
    /// Daemon fencing epoch observed when the transaction began.
    pub daemon_epoch: u64,
    /// Principal on whose behalf the transaction runs.
    pub principal_id: PrincipalId,
    /// Command that caused this transaction.
    pub command_id: CommandId,
    /// Optional correlation identifier for observability.
    pub correlation_id: Option<String>,
}

/// Monotonic daemon fencing epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DaemonEpoch(pub u64);

/// Durable daemon fence: current instance, epoch, and lease expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonFence {
    /// Daemon instance that owns the fence.
    pub instance_id: DaemonInstanceId,
    /// Monotonic fencing epoch.
    pub epoch: DaemonEpoch,
    /// UTC Unix milliseconds at which the fence lease expires.
    pub lease_expires_unix_ms: i64,
}
