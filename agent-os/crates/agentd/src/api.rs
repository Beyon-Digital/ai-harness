//! Control API server wiring.
//!
//! The composition root acquires the [`DaemonLock`] first, binds the control
//! socket under it (stale files are unlinked only once the lock proves the
//! owner is gone), then serves `MvpControlApi` until the drain signal
//! resolves. Held exclusively by the daemon epoch it was started under.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use command_coordinator::CommandCoordinator;
use control_api::{ControlApiService, ControlSocket, HealthInfo, PeerPrincipalMap, serve};
use domain::ids::DaemonInstanceId;
use domain::provider::IdProvider;
use errors::Result;
use kernel_store::KernelStore;

use crate::lock::DaemonLock;

/// Binds and serves the control plane until `shutdown` resolves.
///
/// `lock` must be the live daemon singleton lock for `runtime_dir` — the
/// socket refuses to unlink a file owned by a live endpoint, and the lock
/// guards against a second daemon racing to steal it.
#[allow(clippy::too_many_arguments)]
pub async fn serve_control_api(
    runtime_dir: &Path,
    lock: &DaemonLock,
    coordinator: Arc<CommandCoordinator>,
    store: Arc<dyn KernelStore>,
    ids: Arc<dyn IdProvider>,
    principals: Arc<dyn PeerPrincipalMap>,
    daemon_instance: DaemonInstanceId,
    daemon_epoch: u64,
    event_service: Option<control_api::EventApiService>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let socket = ControlSocket::bind(runtime_dir, lock)?;
    serve(
        socket,
        ControlApiService::new(
            coordinator,
            store,
            ids,
            principals,
            HealthInfo {
                status: "running".to_owned(),
                daemon_instance_id: daemon_instance.to_string(),
                daemon_fencing_epoch: daemon_epoch,
                active_config_generation_id: String::new(),
                outbox_unpublished_count: 0,
            },
        ),
        event_service,
        shutdown,
    )
    .await
}
