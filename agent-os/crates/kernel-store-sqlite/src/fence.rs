//! Durable daemon fence operations.
//!
//! The fence is the singleton row in `daemon_fence`. A claim runs in an
//! explicit `BEGIN IMMEDIATE` transaction so two processes serialize on the
//! SQLite write lock, and either inserts epoch `1` or increments the persisted
//! epoch (R4.2, R4.5). [`assert_epoch`] is the admission check executed by the
//! write-transaction guard before any statement runs: a missing or stale epoch
//! is rejected with `FailedPrecondition`, so the transaction cannot mutate and
//! a daemon that lost its fence refuses new authoritative work (R4.3, R4.4,
//! D6). The persisted row is the single authority; this module stores no other
//! daemon state (D8).

use domain::ids::DaemonInstanceId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::types::{DaemonEpoch, DaemonFence};
use sqlx::SqlitePool;

use crate::mapping;

/// Normative fence lease: `daemon.fence_lease_ms` in `specs/limits.yaml`.
pub const FENCE_LEASE_MS: i64 = 15_000;

/// Claims the singleton fence for `instance` and returns the stored row.
///
/// The first claim creates epoch `1`; every later claim increments the
/// persisted epoch, so a restart or a superseding instance fences out work
/// issued under an older epoch.
pub async fn claim(
    pool: &SqlitePool,
    instance: DaemonInstanceId,
    lease_ms: i64,
) -> errors::Result<DaemonFence> {
    if lease_ms <= 0 {
        return Err(KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "daemon fence lease must be a positive number of milliseconds",
        ));
    }
    let now_ms = mapping::unix_ms_now()?;
    let lease_expires_unix_ms = now_ms.checked_add(lease_ms).ok_or_else(|| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "daemon fence lease exceeds the unix millisecond range",
        )
    })?;
    let mut txn = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(mapping::from_sqlx)?;
    let epoch: i64 = sqlx::query_scalar(
        "INSERT INTO daemon_fence (singleton, instance_id, fencing_epoch, lease_expires_ms, \
         updated_at_ms) VALUES (1, ?1, 1, ?2, ?3) \
         ON CONFLICT(singleton) DO UPDATE SET \
           instance_id = excluded.instance_id, \
           fencing_epoch = fencing_epoch + 1, \
           lease_expires_ms = excluded.lease_expires_ms, \
           updated_at_ms = excluded.updated_at_ms \
         RETURNING fencing_epoch",
    )
    .bind(instance.to_string())
    .bind(lease_expires_unix_ms)
    .bind(now_ms)
    .fetch_one(&mut *txn)
    .await
    .map_err(mapping::from_sqlx)?;
    txn.commit().await.map_err(mapping::from_sqlx)?;
    Ok(DaemonFence {
        instance_id: instance,
        epoch: DaemonEpoch(mapping::decode_u64("daemon_fence.fencing_epoch", epoch)?),
        lease_expires_unix_ms,
    })
}

/// Reads the current fence, if one has ever been claimed.
pub async fn current(pool: &SqlitePool) -> errors::Result<Option<DaemonFence>> {
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT instance_id, fencing_epoch, lease_expires_ms FROM daemon_fence \
         WHERE singleton = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(mapping::from_sqlx)?;
    row.map(|(instance, epoch, lease_expires_unix_ms)| {
        Ok(DaemonFence {
            instance_id: mapping::decode_id("daemon_fence.instance_id", &instance)?,
            epoch: DaemonEpoch(mapping::decode_u64("daemon_fence.fencing_epoch", epoch)?),
            lease_expires_unix_ms,
        })
    })
    .transpose()
}

/// Requires the persisted epoch to equal `expected`.
pub(crate) async fn assert_epoch(
    conn: &mut sqlx::SqliteConnection,
    expected: u64,
) -> errors::Result<()> {
    let persisted: Option<i64> =
        sqlx::query_scalar("SELECT fencing_epoch FROM daemon_fence WHERE singleton = 1")
            .fetch_optional(conn)
            .await
            .map_err(mapping::from_sqlx)?;
    if persisted.is_none() || persisted != i64::try_from(expected).ok() {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "write transaction rejected: daemon epoch is absent or stale",
        ));
    }
    Ok(())
}
