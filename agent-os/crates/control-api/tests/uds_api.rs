//! API-001: socket permissions, lock-bound socket ownership, graceful
//! shutdown, client smoke test, and peer-principal enforcement.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use command_coordinator::handler::{CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use control_api::generated::mvp_control_api_client::MvpControlApiClient;
use control_api::{ControlApiService, ControlSocket, HealthInfo, UidPrincipalMap, serve};
use domain::faults::NoFaults;
use domain::generated::contract::{CommandRequest, HealthRequest};
use domain::ids::{ActorId, CommandId, IdempotencyKey, PrincipalId};
use domain::time::SystemClock;
use kernel_store::KernelStore;
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;

struct EchoHandler;

#[async_trait::async_trait]
impl CommandHandler for EchoHandler {
    async fn handle(
        &self,
        _ctx: &command_coordinator::handler::CommandContext,
        _txn: &mut dyn kernel_store::KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload,
        })
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    runtime_dir: std::path::PathBuf,
    store: Arc<SqliteKernelStore>,
    ids: Arc<dyn domain::provider::IdProvider>,
    coordinator: Arc<CommandCoordinator>,
    principal: PrincipalId,
    actor: ActorId,
    uid: u32,
    epoch: u64,
}

impl Rig {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join("kernel.db"),
                pool_max_connections: 4,
                busy_timeout_ms: 10_000,
            })
            .await
            .expect("store"),
        );
        let ids = DeterministicIds::new(SEED);
        let fence = store
            .acquire_daemon_fence(domain::ids::DaemonInstanceId::new(&ids))
            .await
            .expect("fence");
        let mut registry = CommandRegistry::new();
        registry
            .register("agentos.test.Echo", Arc::new(EchoHandler))
            .expect("register");
        let coordinator = Arc::new(CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            Arc::new(SystemClock),
            Arc::new(NoFaults),
        ));
        Self {
            runtime_dir: dir.path().join("run"),
            store,
            ids: Arc::new(DeterministicIds::new(SEED + 1)),
            _dir: dir,
            coordinator,
            principal: PrincipalId::new(&ids),
            actor: ActorId::new(&ids),
            uid: rustix::process::getuid().as_raw(),
            epoch: fence.epoch.0,
        }
    }

    fn service(&self, mapped_uid: u32) -> ControlApiService {
        let principals =
            Arc::new(UidPrincipalMap::new([(mapped_uid, self.principal)]).expect("map"));
        ControlApiService::new(
            self.coordinator.clone(),
            self.store.clone(),
            self.ids.clone(),
            principals,
            None,
            HealthInfo {
                status: "running".to_owned(),
                daemon_instance_id: "daemon-1".to_owned(),
                daemon_fencing_epoch: self.epoch,
                active_config_generation_id: String::new(),
                outbox_unpublished_count: 0,
            },
        )
    }
}

// A minimal lock token for the socket's proof-of-lock parameter.
#[derive(Debug)]
struct FakeLock;

impl control_api::uds::DaemonLockHeld for FakeLock {}

fn digest_hex(tag: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(tag.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn try_client(
    path: &std::path::Path,
) -> Result<MvpControlApiClient<tonic::transport::Channel>, tonic::transport::Error> {
    let path = path.to_path_buf();
    let channel = tonic::transport::Endpoint::from_static("http://[::1]:0")
        .connect_with_connector(tower::service_fn(move |_| {
            let path = path.clone();
            async move {
                tokio::net::UnixStream::connect(path)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
                    .map_err(std::io::Error::other)
            }
        }))
        .await?;
    Ok(MvpControlApiClient::new(channel))
}

async fn client(path: &std::path::Path) -> MvpControlApiClient<tonic::transport::Channel> {
    try_client(path).await.expect("connect")
}

/// Waits until the socket accepts a gRPC connection.
async fn wait_ready(path: &std::path::Path) {
    for _ in 0..200 {
        if try_client(path).await.is_ok() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("server never became ready at {}", path.display());
}

#[tokio::test]
async fn socket_is_created_with_owner_only_permissions() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let mode = std::fs::metadata(socket.path())
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    drop(socket);
    assert!(!rig.runtime_dir.join("control.sock").exists());
}

#[tokio::test]
async fn second_daemon_cannot_steal_the_socket() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    // A live bound endpoint answers connects, so the second bind refuses
    // rather than unlinking it.
    let err = ControlSocket::bind(&rig.runtime_dir, &FakeLock)
        .map(|_| ())
        .expect_err("second bind must fail");
    assert_eq!(err.code(), errors::codes::ErrorCode::FailedPrecondition);
    drop(socket);
}

#[tokio::test]
async fn stale_socket_is_replaced_only_when_dead() {
    let rig = Rig::new().await;
    std::fs::create_dir_all(&rig.runtime_dir).expect("dir");
    let path = rig.runtime_dir.join("control.sock");
    std::fs::write(&path, b"stale").expect("stale file");
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("rebind");
    assert!(socket.path().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_stops_the_server() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(rig.uid), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;
    let health = c
        .health(HealthRequest {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.daemon_instance_id, "daemon-1");
    assert_eq!(health.daemon_fencing_epoch, rig.epoch);
    stop_tx.send(()).expect("stop");
    server.await.expect("join").expect("serve clean");
    assert!(!path.exists(), "socket file is unlinked on shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_smoke_submit_command_round_trips() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(rig.uid), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;
    let ids = DeterministicIds::new(SEED);
    let payload = vec![0xAA, 0xBB];
    let response = c
        .submit_command(CommandRequest {
            command_id: CommandId::new(&ids).to_string(),
            idempotency_key: IdempotencyKey::new("smoke-1").unwrap().to_string(),
            principal_id: rig.principal.to_string(),
            actor_id: rig.actor.to_string(),
            device_id: String::new(),
            request_digest: digest_hex("smoke-1"),
            correlation_id: String::new(),
            causation_id: String::new(),
            deadline_unix_ms: 0,
            command_type: "agentos.test.Echo".to_owned(),
            payload,
        })
        .await
        .expect("submit")
        .into_inner();
    assert_eq!(response.outcome_code, "ok");
    stop_tx.send(()).expect("stop");
    server.await.expect("join").expect("serve clean");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claimed_principal_must_match_the_peer_principal() {
    let rig = Rig::new().await;
    let other = PrincipalId::new(&DeterministicIds::new(SEED + 9));
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(rig.uid), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;
    let ids = DeterministicIds::new(SEED);
    let status = c
        .submit_command(CommandRequest {
            command_id: CommandId::new(&ids).to_string(),
            idempotency_key: IdempotencyKey::new("mismatch-1").unwrap().to_string(),
            principal_id: other.to_string(), // claims a different principal
            actor_id: rig.actor.to_string(),
            device_id: String::new(),
            request_digest: digest_hex("mismatch-1"),
            correlation_id: String::new(),
            causation_id: String::new(),
            deadline_unix_ms: 0,
            command_type: "agentos.test.Echo".to_owned(),
            payload: Vec::new(),
        })
        .await
        .expect_err("mismatched claim is rejected");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
    stop_tx.send(()).expect("stop");
    server.await.expect("join").expect("serve clean");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_uid_is_rejected() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    // Map a different uid entirely: this peer has no principal.
    let server = tokio::spawn(serve(socket, rig.service(u32::MAX - 1), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;
    let status = c
        .health(HealthRequest {})
        .await
        .expect_err("unmapped peer denied");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
    stop_tx.send(()).expect("stop");
    server.await.expect("join").expect("serve clean");
}
