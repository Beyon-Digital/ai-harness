//! API-004: `agentctl` against a real daemon socket — the CLI must reach
//! every surface through the public Control/Event APIs only.

use std::str::FromStr;
use std::sync::Arc;

use command_coordinator::handler::CommandRegistry;
use command_coordinator::{CommandCoordinator, FixedFence};
use control_api::{
    ControlApiService, ControlSocket, EventApiService, HealthInfo, UidPrincipalMap, serve,
};
use domain::faults::NoFaults;
use domain::ids::PrincipalId;
use domain::time::SystemClock;
use kernel_store::KernelStore;
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;

#[derive(Debug)]
struct FakeLock;

struct Rig {
    _dir: TempDir,
    socket_path: std::path::PathBuf,
    stop: tokio::sync::oneshot::Sender<()>,
    _server: tokio::task::JoinHandle<errors::Result<()>>,
}

impl Rig {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("dir");
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
            .acquire_daemon_fence(domain::ids::DaemonInstanceId::new(ids.as_ref()))
            .await
            .expect("fence");
        let uid = rustix::process::geteuid().as_raw();
        let principal = PrincipalId::from_str("00000000-0000-7000-8000-000000000001").unwrap();

        let mut registry = CommandRegistry::new();
        runtime::register_handlers(
            &mut registry,
            runtime::RuntimeDeps {
                clock: Arc::new(SystemClock),
                ids: ids.clone(),
            },
        )
        .expect("runtime handlers");
        approvals::register_handlers(
            &mut registry,
            approvals::ApprovalDeps {
                clock: Arc::new(SystemClock),
            },
        )
        .expect("approval handlers");
        config_engine::register_handlers(
            &mut registry,
            config_engine::ConfigDeps {
                ids: ids.clone(),
                clock: Arc::new(SystemClock),
                running_services: {
                    let doc = std::fs::read(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("../../config/default.yaml"),
                    )
                    .expect("default config");
                    let doc =
                        config_engine::parse_document(std::str::from_utf8(&doc).expect("utf8"))
                            .expect("default config parses");
                    config_engine::generation_services(&doc)
                },
            },
        )
        .expect("config handlers");

        let coordinator = Arc::new(CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            Arc::new(SystemClock),
            Arc::new(NoFaults),
        ));
        let journal = Arc::new(
            event_journal_sqlite::SqliteEventJournal::open(event_journal_sqlite::JournalConfig {
                path: dir.path().join("events.db"),
                busy_timeout_ms: 5_000,
            })
            .await
            .expect("journal"),
        );
        let principals = Arc::new(UidPrincipalMap::new([(uid, principal)]).expect("map"));
        let service = ControlApiService::new(
            coordinator,
            store,
            ids,
            principals.clone(),
            HealthInfo {
                status: "running".to_owned(),
                daemon_instance_id: "test-daemon".to_owned(),
                daemon_fencing_epoch: 1,
                active_config_generation_id: String::new(),
                outbox_unpublished_count: 0,
            },
        );
        let events = EventApiService::new(
            journal,
            Arc::new(events::live_bus::LiveBus::new(64)),
            principals,
        );
        let runtime_dir = dir.path().join("run");
        let socket = ControlSocket::bind(&runtime_dir, &FakeLock).expect("bind");
        let socket_path = socket.path().to_path_buf();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(socket, service, Some(events), async move {
            let _ = rx.await;
        }));
        // Wait for the socket to exist.
        for _ in 0..200 {
            if socket_path.exists() {
                break;
            }
            std::thread::yield_now();
            tokio::task::yield_now().await;
        }
        assert!(socket_path.exists(), "daemon socket never appeared");
        Self {
            _dir: dir,
            socket_path,
            stop: tx,
            _server: server,
        }
    }

    async fn cli(&self, args: &[&str]) -> serde_json::Value {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        agentctl::run(&owned, &self.socket_path)
            .await
            .unwrap_or_else(|e| panic!("agentctl {args:?} failed: {e}"))
    }
}

fn strs(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|s| s.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_and_session_over_socket() {
    let rig = Rig::new().await;
    let health = rig.cli(&["health"]).await;
    assert_eq!(health["status"], "running");
    assert_eq!(health["daemon_instance_id"], "test-daemon");

    let session = rig.cli(&["create-session"]).await;
    assert_eq!(session["outcome_code"], "ok");
    let session_id = session["payload"].as_str().unwrap().to_string();
    assert!(!session_id.is_empty(), "session id from command payload");
    rig.stop.send(()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_then_config_lifecycle_and_approvals() {
    let rig = Rig::new().await;

    // Config lifecycle through commands only — the canonical default doc.
    let doc = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/default.yaml");
    let proposed = rig
        .cli(&["config", "propose", "--file", doc.to_str().unwrap()])
        .await;
    assert_eq!(proposed["outcome_code"], "ok");
    let gen_id = proposed["payload"].as_str().unwrap().to_string();

    let tested = rig
        .cli(&["config", "test", "--generation-id", &gen_id])
        .await;
    assert_eq!(tested["payload"], "passed");

    let activated = rig
        .cli(&["config", "activate", "--generation-id", &gen_id])
        .await;
    assert_eq!(activated["outcome_code"], "ok");

    let shown = rig.cli(&["config", "show"]).await;
    assert_eq!(shown["generation_id"], gen_id);

    // Approvals list via the read API.
    let approvals = rig.cli(&["approvals", "list"]).await;
    assert!(approvals["approvals"].is_array());
    rig.stop.send(()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_found_and_json_parseable() {
    let rig = Rig::new().await;
    // Unknown run surfaces a clean JSON error from the lib.
    let err = agentctl::run(
        &strs(&["get-run", "00000000-0000-7000-8000-000000000099"]),
        &rig.socket_path,
    )
    .await;
    assert!(err.is_err(), "missing run must error");
    let adapters = rig.cli(&["adapters"]).await;
    assert!(adapters["adapters"].is_array());
    let events = rig
        .cli(&["events", "read", "--stream-key", "config/global"])
        .await;
    assert!(events["events"].is_array());
    rig.stop.send(()).unwrap();
}
