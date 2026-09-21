//! Durable adapter-instance record for a spawned child.
//!
//! Every spawn persists an `adapter_instances` row (`starting`) carrying the
//! exact adapter identity, daemon instance, pid, and process start identity;
//! a successful handshake moves it to `ready`, exit to `exited`, and an
//! abnormal or handshake-failed end to `failed`. A restart inserts a new row
//! — the prior instance is never mutated in place, so stale protocol
//! sessions stay invalid (process-supervisor.md).
#![forbid(unsafe_code)]

use domain::ids::AdapterInstanceId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{AdapterInstanceStatePatch, NewAdapterInstance};

use crate::spawn::{Child, ExitReason, SpawnSpec};

/// Instance lifecycle literals (schema CHECK).
pub mod instance_state {
    /// Spawned, handshake pending.
    pub const STARTING: &str = "starting";
    /// Handshake verified; protocol endpoint live.
    pub const READY: &str = "ready";
    /// Exited cleanly or after shutdown.
    pub const EXITED: &str = "exited";
    /// Crashed or failed verification.
    pub const FAILED: &str = "failed";
}

/// Inserts the `starting` instance row inside the caller's transaction.
/// Call after [`crate::spawn::spawn`] so pid/start identity are captured.
pub async fn record_spawn(
    txn: &mut dyn KernelTxn,
    spec: &SpawnSpec,
    child: &Child,
    now_ms: i64,
) -> errors::Result<()> {
    txn.adapters()
        .insert_instance(NewAdapterInstance {
            adapter_instance_id: spec.adapter_instance_id,
            adapter_id: spec.adapter_id,
            adapter_version: spec.adapter_version.clone(),
            bundle_digest: spec.expected_bundle_digest.clone(),
            daemon_instance_id: spec.daemon_instance_id,
            pid: Some(i64::from(child.pid)),
            process_start_identity: Some(child.start_identity.clone()),
            state: instance_state::STARTING.to_owned(),
            exit_reason: None,
            last_heartbeat_ms: Some(now_ms),
            started_at_ms: now_ms,
            ended_at_ms: None,
        })
        .await
}

/// `starting -> ready` after a verified handshake.
pub async fn mark_ready(
    txn: &mut dyn KernelTxn,
    instance: AdapterInstanceId,
    now_ms: i64,
) -> errors::Result<()> {
    if cas(
        txn,
        instance,
        instance_state::STARTING,
        AdapterInstanceStatePatch {
            state: Some(instance_state::READY.to_owned()),
            last_heartbeat_ms: Some(now_ms),
            ..AdapterInstanceStatePatch::default()
        },
    )
    .await?
    {
        Ok(())
    } else {
        Err(state_conflict(instance))
    }
}

/// `starting|ready -> exited` with the normalized exit reason.
pub async fn mark_exited(
    txn: &mut dyn KernelTxn,
    instance: AdapterInstanceId,
    reason: &ExitReason,
    now_ms: i64,
) -> errors::Result<()> {
    let patch = AdapterInstanceStatePatch {
        state: Some(instance_state::EXITED.to_owned()),
        exit_reason: Some(reason.to_string()),
        ended_at_ms: Some(now_ms),
        ..AdapterInstanceStatePatch::default()
    };
    if cas(txn, instance, instance_state::READY, patch.clone()).await?
        || cas(txn, instance, instance_state::STARTING, patch).await?
    {
        return Ok(());
    }
    Err(state_conflict(instance))
}

/// `starting|ready -> failed` for handshake failures and abnormal exits.
pub async fn mark_failed(
    txn: &mut dyn KernelTxn,
    instance: AdapterInstanceId,
    reason: &str,
    now_ms: i64,
) -> errors::Result<()> {
    let patch = AdapterInstanceStatePatch {
        state: Some(instance_state::FAILED.to_owned()),
        exit_reason: Some(reason.to_owned()),
        ended_at_ms: Some(now_ms),
        ..AdapterInstanceStatePatch::default()
    };
    if cas(txn, instance, instance_state::READY, patch.clone()).await?
        || cas(txn, instance, instance_state::STARTING, patch).await?
    {
        return Ok(());
    }
    Err(state_conflict(instance))
}

/// Heartbeat update under the current state.
pub async fn heartbeat(
    txn: &mut dyn KernelTxn,
    instance: AdapterInstanceId,
    state: &str,
    now_ms: i64,
) -> errors::Result<()> {
    if cas(
        txn,
        instance,
        state,
        AdapterInstanceStatePatch {
            last_heartbeat_ms: Some(now_ms),
            ..AdapterInstanceStatePatch::default()
        },
    )
    .await?
    {
        Ok(())
    } else {
        Err(state_conflict(instance))
    }
}

async fn cas(
    txn: &mut dyn KernelTxn,
    instance: AdapterInstanceId,
    expect_state: &str,
    patch: AdapterInstanceStatePatch,
) -> errors::Result<bool> {
    txn.adapters()
        .cas_instance_state(instance, expect_state, patch)
        .await
}

fn state_conflict(instance: AdapterInstanceId) -> KernelError {
    KernelError::new(
        ErrorCode::Conflict,
        RetryClass::Safe,
        format!("adapter instance {instance} lost a state race"),
    )
}
