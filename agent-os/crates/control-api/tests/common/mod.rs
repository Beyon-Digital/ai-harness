//! Shared test rig for control/event API integration tests.
#![allow(dead_code)]

use sha2::Digest;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use command_coordinator::handler::{CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use control_api::generated::mvp_control_api_client::MvpControlApiClient;
use control_api::{ControlApiService, HealthInfo, UidPrincipalMap};
use domain::faults::NoFaults;
use domain::generated::contract::CommandRequest;
use domain::ids::{ActorId, CommandId, DaemonInstanceId, IdempotencyKey, PrincipalId};
use domain::time::SystemClock;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

pub const SEED: i64 = 1_700_000_000_000;
pub const NOW: i64 = 1_700_000_000_000;

pub fn digest_hex(tag: &str) -> String {
    sha2::Sha256::digest(tag.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub struct Rig {
    pub _dir: TempDir,
    pub runtime_dir: std::path::PathBuf,
    pub store: Arc<SqliteKernelStore>,
    pub ids: Arc<DeterministicIds>,
    pub epoch: u64,
    pub coordinator: Arc<CommandCoordinator>,
    pub principal: PrincipalId,
    pub actor: ActorId,
    pub uid: u32,
    pub calls: Arc<AtomicU64>,
}

impl Rig {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join("k.db"),
                pool_max_connections: 4,
                busy_timeout_ms: 5_000,
            })
            .await
            .expect("store"),
        );
        let ids = Arc::new(DeterministicIds::new(SEED));
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(ids.as_ref()))
            .await
            .expect("fence");
        let calls = Arc::new(AtomicU64::new(0));
        let mut registry = CommandRegistry::new();
        registry
            .register(
                "agentos.test.Counted",
                Arc::new(CountedHandler(calls.clone())),
            )
            .expect("register");
        approvals::register_handlers(
            &mut registry,
            approvals::ApprovalDeps {
                clock: Arc::new(SystemClock),
            },
        )
        .expect("approval handlers");
        let coordinator = Arc::new(CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            Arc::new(SystemClock),
            Arc::new(NoFaults),
        ));
        let principal = PrincipalId::new(ids.as_ref());
        let actor = ActorId::new(ids.as_ref());
        Self {
            runtime_dir: dir.path().join("run"),
            store,
            ids,
            epoch: fence.epoch.0,
            coordinator,
            principal,
            actor,
            uid: rustix::process::getuid().as_raw(),
            calls,
            _dir: dir,
        }
    }

    pub async fn write_txn(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(TxContext {
                daemon_epoch: self.epoch,
                principal_id: self.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await
            .expect("txn")
    }

    pub fn service(&self) -> ControlApiService {
        let principals = Arc::new(UidPrincipalMap::new([(self.uid, self.principal)]).expect("map"));
        ControlApiService::new(
            self.coordinator.clone(),
            self.store.clone(),
            self.ids.clone(),
            principals,
            HealthInfo {
                status: "running".to_owned(),
                daemon_instance_id: "daemon-1".to_owned(),
                daemon_fencing_epoch: 7,
                active_config_generation_id: "gen-9".to_owned(),
                outbox_unpublished_count: 3,
            },
        )
    }
}

pub struct CountedHandler(Arc<AtomicU64>);

#[async_trait::async_trait]
impl CommandHandler for CountedHandler {
    async fn handle(
        &self,
        _ctx: &command_coordinator::handler::CommandContext,
        _txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload,
        })
    }
}

pub async fn client(path: &std::path::Path) -> MvpControlApiClient<tonic::transport::Channel> {
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
        .await
        .expect("connect");
    MvpControlApiClient::new(channel)
}

pub async fn wait_ready(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("socket did not appear");
}

#[derive(Debug)]
pub struct FakeLock;

pub fn counted_envelope(rig: &Rig, key: &str, payload: Vec<u8>) -> CommandRequest {
    CommandRequest {
        command_id: CommandId::new(rig.ids.as_ref()).to_string(),
        idempotency_key: IdempotencyKey::new(key).unwrap().to_string(),
        principal_id: rig.principal.to_string(),
        actor_id: rig.actor.to_string(),
        device_id: String::new(),
        request_digest: digest_hex(key),
        correlation_id: String::new(),
        causation_id: String::new(),
        deadline_unix_ms: 0,
        command_type: "agentos.test.Counted".to_owned(),
        payload,
    }
}
