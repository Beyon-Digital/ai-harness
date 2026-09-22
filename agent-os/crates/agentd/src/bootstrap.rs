//! `agentd` composition root (INT-001): opens the durable store and journal,
//! acquires the singleton lock + daemon fence, runs startup recovery,
//! registers adapter bundles and the initial config generation, then serves
//! the Control/Event APIs and runs the background workers until a drain.
//!
//! Shutdown is drain-first: workers stop taking new work, in-flight turns
//! finish or their issued turns recover as stale on next boot, the API
//! stops accepting connections, the outbox drains, then the lock releases.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use adapter_registry::manifest::parse_manifest;
use adapter_registry::registry;
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandOutcome, CommandRegistry};
use command_coordinator::{CommandCoordinator, FixedFence};
use control_api::{
    ControlApiService, ControlSocket, EventApiService, HealthInfo, PeerPrincipalMap,
};
use domain::faults::NoFaults;
use domain::generated::contract;
use domain::ids::{
    ActorId, CapabilityGrantId, CommandId, DaemonInstanceId, DelegationChainId, IdempotencyKey,
    PrincipalId, RunId,
};
use domain::provider::{IdProvider, SystemIdProvider};
use domain::security::TrustState;
use domain::time::SystemClock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::{KernelStore, NewCapabilityGrant, NewDelegationHop, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::lock::DaemonLock;
use crate::recovery::run_startup_recovery;
use crate::workers::outbox::{EpochSource, OutboxWorker};
use crate::workers::runs::{
    AdapterBundle, RunCancelCompleteHandler, RunWaitExpiredHandler, RunWorker, RunWorkerDeps,
};
use crate::workers::scheduler::{SchedulerWorker, SchedulerWorkerDeps};

/// Boot configuration for the daemon.
pub struct DaemonConfig {
    /// Runtime directory holding the DBs, lock, and control socket.
    pub runtime_dir: PathBuf,
    /// Optional initial config document to activate when none is active.
    pub config_doc: Option<PathBuf>,
    /// Adapter bundle directories to register (manifest + lock + files).
    pub adapter_bundles: Vec<PathBuf>,
    /// Per-run `FIXTURE_LOOP_SCRIPT` values keyed by run id.
    pub loop_scripts: HashMap<RunId, String>,
    /// Extra env vars injected into spawned loop-adapter processes.
    pub loop_env: HashMap<String, String>,
    /// Extra env vars injected into spawned effect-adapter processes.
    pub effect_env: HashMap<String, String>,
    /// Worker poll interval.
    pub poll: Duration,
    /// Log as JSON when true.
    pub json_logs: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            runtime_dir: PathBuf::from("run"),
            config_doc: None,
            adapter_bundles: Vec::new(),
            loop_scripts: HashMap::new(),
            loop_env: HashMap::new(),
            effect_env: HashMap::new(),
            poll: Duration::from_millis(50),
            json_logs: false,
        }
    }
}

/// A running daemon; `wait` blocks until shutdown completes.
pub struct Daemon {
    socket_path: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    workers: Vec<JoinHandle<()>>,
    server: JoinHandle<errors::Result<()>>,
    store: Arc<SqliteKernelStore>,
    epoch: u64,
    ids: Arc<dyn IdProvider>,
    // Held for the daemon's lifetime: releasing it would drop the OS lock
    // that proves this process is the store's single writer.
    _lock: DaemonLock,
}

impl Daemon {
    /// The control socket path clients connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The composed kernel store — used by integration tests to drive
    /// services that have no control-API verb (workspace coordinator).
    pub fn store(&self) -> &Arc<SqliteKernelStore> {
        &self.store
    }

    /// The fencing epoch this daemon instance holds.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The daemon's id provider.
    pub fn ids(&self) -> Arc<dyn IdProvider> {
        self.ids.clone()
    }

    /// Signals drain-first shutdown.
    pub fn initiate_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Aborts every worker and the API server without draining — the
    /// crash simulation a recovery test needs (kernel store, lock file,
    /// and all committed rows survive; in-flight work does not). Blocks
    /// until the control socket is unconnectable so an immediate reboot
    /// cannot race the dying listener.
    pub async fn abort(self) {
        for worker in &self.workers {
            worker.abort();
        }
        self.server.abort();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::os::unix::net::UnixStream::connect(&self.socket_path).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "aborted daemon's socket stayed live"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Waits for workers and the API server to finish draining.
    pub async fn wait(self) -> errors::Result<()> {
        for worker in self.workers {
            let _ = worker.await;
        }
        let result = self.server.await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(boot_error("api task panicked")),
        }
    }
}

/// Boots the daemon inside `config.runtime_dir`.
pub async fn boot(config: DaemonConfig) -> errors::Result<Daemon> {
    let _ = observability::init_tracing(config.json_logs);
    std::fs::create_dir_all(&config.runtime_dir).map_err(|e| io("create runtime dir", &e))?;
    let runtime_dir = config.runtime_dir.clone();

    // R4.1/R4.6: the OS lock is the single-writer gate; a second daemon
    // refuses to start rather than waiting.
    let lock = DaemonLock::acquire(&runtime_dir).map_err(|error| {
        KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("{error}"),
        )
    })?;

    let ids: Arc<dyn IdProvider> = Arc::new(SystemIdProvider);
    let clock: Arc<dyn domain::time::Clock> = Arc::new(SystemClock);
    let store = Arc::new(
        SqliteKernelStore::open(StoreConfig {
            path: runtime_dir.join("kernel.db"),
            pool_max_connections: 4,
            busy_timeout_ms: 5_000,
        })
        .await?,
    );
    let daemon_instance = DaemonInstanceId::new(ids.as_ref());
    let fence = store.acquire_daemon_fence(daemon_instance).await?;
    let epoch = fence.epoch.0;

    let report = run_startup_recovery(store.as_ref(), clock.clone()).await?;
    tracing::info!(?report, epoch, "startup recovery complete");

    // Command registry: public command catalog + daemon-internal commands.
    let running_services = match config.config_doc.as_ref() {
        Some(path) => {
            let doc = std::fs::read(path).map_err(|e| io("read config doc", &e))?;
            let doc = config_engine::parse_document(
                std::str::from_utf8(&doc)
                    .map_err(|_| boot_error("config document is not utf-8"))?,
            )?;
            Some((path.clone(), config_engine::generation_services(&doc)))
        }
        None => None,
    };
    let mut registry = CommandRegistry::new();
    runtime::register_handlers(
        &mut registry,
        runtime::RuntimeDeps {
            clock: clock.clone(),
            ids: ids.clone(),
        },
    )?;
    approvals::register_handlers(
        &mut registry,
        approvals::ApprovalDeps {
            clock: clock.clone(),
        },
    )?;
    config_engine::register_handlers(
        &mut registry,
        config_engine::ConfigDeps {
            ids: ids.clone(),
            clock: clock.clone(),
            running_services: running_services
                .as_ref()
                .map(|(_, services)| services.clone())
                .unwrap_or_default(),
        },
    )?;
    registry.register(
        crate::workers::runs::CMD_RUN_WAIT_EXPIRED,
        Arc::new(RunWaitExpiredHandler),
    )?;
    registry.register(
        crate::workers::runs::CMD_RUN_CANCEL_COMPLETE,
        Arc::new(RunCancelCompleteHandler::new(ids.clone())),
    )?;

    let coordinator = Arc::new(CommandCoordinator::new(
        store.clone(),
        Arc::new(registry),
        Arc::new(FixedFence(epoch)),
        clock.clone(),
        Arc::new(NoFaults),
    ));

    let journal = Arc::new(
        event_journal_sqlite::SqliteEventJournal::open(event_journal_sqlite::JournalConfig {
            path: runtime_dir.join("events.db"),
            busy_timeout_ms: 5_000,
        })
        .await?,
    );
    let bus = Arc::new(events::live_bus::LiveBus::new(64));

    // System principal/actor for worker-internal commands.
    let principal = PrincipalId::from_str("00000000-0000-7000-8000-0000000000dd")
        .expect("fixed daemon principal");
    let actor =
        ActorId::from_str("00000000-0000-7000-8000-0000000000ae").expect("fixed daemon actor");
    let operator_chain = seed_operator_chain(
        &store,
        ids.as_ref(),
        epoch,
        clock.now_unix_ms(),
        principal,
        actor,
    )
    .await?;

    // Adapter bundle registration + spawn directory index. Beyond the
    // config-supplied bundles, every *enabled* bundle installed via
    // `agentd adapter install` under the runtime dir is registered — a
    // corrupt installed bundle warns and skips rather than wedging boot.
    let mut bundles = Vec::new();
    for dir in &config.adapter_bundles {
        let bundle =
            register_bundle(store.clone(), dir, ids.as_ref(), epoch, clock.now_unix_ms()).await?;
        bundles.push(bundle);
    }
    match crate::adapters::enabled_bundle_dirs(&runtime_dir) {
        Ok(installed) => {
            for dir in installed {
                if config.adapter_bundles.iter().any(|d| d == &dir) {
                    continue;
                }
                match register_bundle(
                    store.clone(),
                    &dir,
                    ids.as_ref(),
                    epoch,
                    clock.now_unix_ms(),
                )
                .await
                {
                    Ok(bundle) => bundles.push(bundle),
                    Err(error) => {
                        tracing::warn!(dir = %dir.display(), %error, "installed adapter skipped")
                    }
                }
            }
        }
        Err(error) => tracing::warn!(%error, "installed adapter index unreadable — skipped"),
    }

    // Activate the initial config generation when none is active.
    let active_generation = if let Some((path, _)) = &running_services {
        bootstrap_config(
            store.clone(),
            coordinator.clone(),
            ids.clone(),
            clock.clone(),
            principal,
            actor,
            path,
        )
        .await?
    } else {
        read_active_generation(store.clone()).await?
    };

    struct EpochHolder(u64);
    impl EpochSource for EpochHolder {
        fn epoch(&self) -> u64 {
            self.0
        }
    }
    let epoch_source: Arc<dyn EpochSource> = Arc::new(EpochHolder(epoch));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let workers: Vec<JoinHandle<()>> = vec![
        tokio::spawn(
            OutboxWorker::new(
                Arc::new(events::dispatcher::EventDispatcher::new(
                    store.clone(),
                    journal.clone(),
                    bus.clone(),
                    Arc::new(NoFaults),
                    clock.clone(),
                    ids.clone(),
                    principal,
                )),
                epoch_source.clone(),
                config.poll,
            )
            .run(shutdown_rx.clone()),
        ),
        tokio::spawn(
            SchedulerWorker::new(
                SchedulerWorkerDeps {
                    store: store.clone(),
                    coordinator: coordinator.clone(),
                    epoch: epoch_source.clone(),
                    ids: ids.clone(),
                    clock: clock.clone(),
                    principal,
                    actor,
                },
                config.poll,
            )
            .run(shutdown_rx.clone()),
        ),
        tokio::spawn(
            RunWorker::new(
                RunWorkerDeps {
                    store: store.clone(),
                    coordinator: coordinator.clone(),
                    epoch: epoch_source,
                    ids: ids.clone(),
                    clock: clock.clone(),
                    daemon_instance,
                    principal,
                    actor,
                    bundles,
                    loop_scripts: Arc::new(config.loop_scripts.clone()),
                    loop_env: config.loop_env.clone(),
                    runtime_dir: config.runtime_dir.clone(),
                    effect_env: config.effect_env.clone(),
                },
                config.poll,
            )
            .run(shutdown_rx.clone()),
        ),
    ];

    let uid = current_uid();
    let principals: Arc<dyn PeerPrincipalMap> = Arc::new(
        control_api::UidPrincipalMap::new([(uid, principal)]).map_err(|error| {
            KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                format!("{error}"),
            )
        })?,
    );
    let service = ControlApiService::new(
        coordinator,
        store.clone(),
        ids.clone(),
        principals.clone(),
        Some(operator_chain),
        HealthInfo {
            status: "running".to_owned(),
            daemon_instance_id: daemon_instance.to_string(),
            daemon_fencing_epoch: epoch,
            active_config_generation_id: active_generation.unwrap_or_default(),
            outbox_unpublished_count: 0,
        },
    );
    let events_service = EventApiService::new(journal, bus, principals);
    let socket = ControlSocket::bind(&runtime_dir, &lock).map_err(|error| {
        KernelError::new(ErrorCode::Internal, RetryClass::Never, format!("{error}"))
    })?;
    let socket_path = socket.path().to_path_buf();
    let server_shutdown = {
        let shutdown_rx = shutdown_rx.clone();
        async move {
            let mut rx = shutdown_rx;
            loop {
                if rx.changed().await.is_err() || *rx.borrow() {
                    return;
                }
            }
        }
    };
    let server = tokio::spawn(control_api::serve(
        socket,
        service,
        Some(events_service),
        server_shutdown,
    ));

    tracing::info!(socket = %socket_path.display(), epoch, "agentd ready");
    Ok(Daemon {
        socket_path,
        shutdown_tx,
        workers,
        server,
        store,
        epoch,
        ids,
        _lock: lock,
    })
}

/// Registers one adapter bundle dir (idempotent across restarts: an
/// identical registration replays to the stored row).
async fn register_bundle(
    store: Arc<SqliteKernelStore>,
    dir: &Path,
    ids: &dyn IdProvider,
    epoch: u64,
    now_ms: i64,
) -> errors::Result<AdapterBundle> {
    let manifest_bytes = std::fs::read(dir.join(adapter_registry::manifest::MANIFEST_FILE))
        .map_err(|e| io("read adapter manifest", &e))?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let registration = {
        let mut txn = store
            .begin_write(kernel_store::TxContext {
                daemon_epoch: epoch,
                principal_id: PrincipalId::from_str("00000000-0000-7000-8000-0000000000dd")
                    .expect("daemon principal"),
                command_id: domain::ids::CommandId::new(ids),
                correlation_id: None,
            })
            .await?;
        let outcome = registry::register(&mut *txn, dir, TrustState::Trusted, now_ms).await;
        match outcome {
            Ok(row) => {
                txn.commit().await?;
                row
            }
            Err(error) if error.code() == ErrorCode::Conflict => {
                txn.rollback().await.ok();
                // Already registered — reload the stored row.
                let mut txn = store.begin_read().await?;
                let manifest_id = manifest
                    .id
                    .parse::<domain::ids::AdapterId>()
                    .map_err(|_| boot_error("manifest id is not an adapter id"))?;
                txn.adapters()
                    .get_registration(manifest_id, &manifest.version, &compute_digest(dir)?)
                    .await?
                    .ok_or_else(|| {
                        boot_error("registered adapter identity could not be reloaded")
                    })?
            }
            Err(error) => return Err(error),
        }
    };
    tracing::info!(
        adapter = %registration.adapter_id,
        version = %registration.version,
        "adapter bundle registered"
    );
    Ok(AdapterBundle {
        adapter_id: registration.adapter_id.to_string(),
        version: registration.version,
        dir: dir.to_path_buf(),
        bundle_digest: registration.bundle_digest,
        isolation: match manifest.runtime.isolation.as_deref() {
            Some("user-ns") => {
                process_supervisor::spawn::Isolation::UserNamespace { network: true }
            }
            Some("user-ns-no-net") => {
                process_supervisor::spawn::Isolation::UserNamespace { network: false }
            }
            _ => process_supervisor::spawn::Isolation::None,
        },
        runtime_type: manifest.runtime.runtime_type.clone(),
    })
}

fn compute_digest(dir: &Path) -> errors::Result<String> {
    let manifest_bytes = std::fs::read(dir.join(adapter_registry::manifest::MANIFEST_FILE))
        .map_err(|e| io("read adapter manifest", &e))?;
    let manifest_digest = adapter_registry::manifest::manifest_digest(&manifest_bytes);
    adapter_registry::registry::compute_bundle_digest(dir, &manifest_digest)
}

/// Proposes, smoke-tests, and activates `path` as the config
/// generation. With no active generation the file seeds the runtime;
/// with one, a *different* document activates as a new generation —
/// a restarted `--config` is a deliberate operator transition, while
/// an identical file is an idempotent no-op.
async fn bootstrap_config(
    store: Arc<SqliteKernelStore>,
    coordinator: Arc<CommandCoordinator>,
    ids: Arc<dyn IdProvider>,
    clock: Arc<dyn domain::time::Clock>,
    principal: PrincipalId,
    actor: ActorId,
    path: &Path,
) -> errors::Result<Option<String>> {
    let document = std::fs::read(path).map_err(|e| io("read config doc", &e))?;
    let digest = format!(
        "sha256:{}",
        Sha256::digest(&document)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut expected_revision = 0u64;
    if let Some(active_id) = read_active_generation(store.clone()).await? {
        let mut txn = store.begin_read().await?;
        let active = txn
            .config()
            .get_generation(
                domain::ids::ConfigGenerationId::from_str(&active_id)
                    .map_err(|_| boot_error("active generation id malformed"))?,
            )
            .await?;
        let pointer = txn.config().get_active().await?;
        if let Some(gen_row) = &active
            && gen_row.digest == digest
        {
            tracing::info!(
                generation_id = active_id,
                "config document unchanged — keeping active generation"
            );
            return Ok(Some(active_id));
        }
        expected_revision = pointer.map(|p| p.revision).unwrap_or(0);
        tracing::info!(
            generation_id = active_id,
            "config document changed — proposing a new generation"
        );
    }
    let outcome = submit(
        &coordinator,
        ids.as_ref(),
        principal,
        actor,
        config_engine::commands::CMD_PROPOSE_CONFIG,
        contract::ProposeConfigGeneration {
            generation_id: String::new(),
            document_bytes: document,
            digest,
        }
        .encode_to_vec(),
        "config.propose.boot".to_owned(),
    )
    .await?;
    let generation_id = String::from_utf8(outcome.payload)
        .map_err(|_| boot_error("propose outcome is not a generation id"))?;
    submit(
        &coordinator,
        ids.as_ref(),
        principal,
        actor,
        config_engine::commands::CMD_MARK_CONFIG_TESTED,
        contract::MarkConfigTested {
            generation_id: generation_id.clone(),
            digest: String::new(),
            test_report_digest: String::new(),
            test_result: String::new(),
        }
        .encode_to_vec(),
        format!("config.test.{generation_id}"),
    )
    .await?;
    submit(
        &coordinator,
        ids.as_ref(),
        principal,
        actor,
        config_engine::commands::CMD_ACTIVATE_CONFIG,
        contract::ActivateConfigGeneration {
            generation_id: generation_id.clone(),
            expected_active_revision: expected_revision,
        }
        .encode_to_vec(),
        format!("config.activate.{generation_id}"),
    )
    .await?;
    let _ = clock;
    tracing::info!(generation_id, "config generation activated");
    Ok(Some(generation_id))
}

/// Reads the active generation id, when one is set.
async fn read_active_generation(store: Arc<SqliteKernelStore>) -> errors::Result<Option<String>> {
    let mut txn = store.begin_read().await?;
    Ok(txn
        .config()
        .get_active()
        .await?
        .map(|active| active.generation_id.to_string()))
}

/// Submits a daemon-internal command through the coordinator.
async fn submit(
    coordinator: &Arc<CommandCoordinator>,
    ids: &dyn IdProvider,
    principal: PrincipalId,
    actor: ActorId,
    command_type: &str,
    payload: Vec<u8>,
    idempotency_key: String,
) -> errors::Result<CommandOutcome> {
    let digest_hex: String = Sha256::digest(&payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    coordinator
        .execute(CommandEnvelope {
            command_id: domain::ids::CommandId::new(ids),
            idempotency_key: IdempotencyKey::new(idempotency_key)
                .map_err(|_| boot_error("internal idempotency key malformed"))?,
            principal_id: principal,
            actor_id: actor,
            device_id: None,
            delegation_chain_id: None,
            request_digest: RequestDigest::from_str(&digest_hex)
                .expect("sha256 hex is a valid request digest"),
            correlation_id: None,
            causation_id: None,
            deadline_unix_ms: None,
            command_type: command_type.to_owned(),
            payload,
        })
        .await
}

/// Derives a deterministic UUIDv7-shaped id from a seed — the operator
/// chain/grant must be identical across daemon restarts for the same owner.
fn deterministic_uuid_v7(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = 0x70 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// The local socket owner is the daemon's operator. Seeds a self-issued
/// single-hop chain granting `effect.resolve_unknown` — the recorded exit
/// from `BlockedUnknownEffect` requires a delegation chain, and local-peer
/// auth derives only the owner principal, so the daemon itself must issue
/// the operator's authority. Idempotent: the ids are content-derived and an
/// existing chain is left untouched.
async fn seed_operator_chain(
    store: &SqliteKernelStore,
    ids: &dyn IdProvider,
    epoch: u64,
    now_ms: i64,
    principal: PrincipalId,
    actor: ActorId,
) -> errors::Result<DelegationChainId> {
    let chain_id = DelegationChainId::from_str(&deterministic_uuid_v7(&format!(
        "agentos/operator-chain/{principal}"
    )))
    .map_err(|_| boot_error("derived operator chain id is invalid"))?;
    let grant_id = CapabilityGrantId::from_str(&deterministic_uuid_v7(&format!(
        "agentos/operator-grant/{principal}"
    )))
    .map_err(|_| boot_error("derived operator grant id is invalid"))?;
    let mut txn = store
        .begin_write(TxContext {
            daemon_epoch: epoch,
            principal_id: principal,
            command_id: CommandId::new(ids),
            correlation_id: None,
        })
        .await?;
    if txn
        .security()
        .list_delegation_hops(chain_id)
        .await?
        .is_empty()
    {
        txn.security()
            .insert_grant(NewCapabilityGrant {
                grant_id,
                principal_id: principal,
                actor_id: actor,
                run_id: None,
                capability_id: "effect.resolve_unknown".to_owned(),
                scope: Vec::new(),
                delegated_from_grant_id: None,
                expires_at_ms: None,
                revoked_at_ms: None,
                created_at_ms: now_ms,
            })
            .await?;
        txn.security()
            .insert_delegation_hop(NewDelegationHop {
                chain_id,
                hop_index: 0,
                principal_or_actor_id: actor.to_string(),
                run_id: None,
                capability_grant_ids: grant_id.to_hyphenated().into_bytes(),
            })
            .await?;
    }
    txn.commit().await?;
    Ok(chain_id)
}

fn current_uid() -> u32 {
    // rustix is a dev-dep only in some builds; fall back to libc-free
    // detection through /proc/self/loginuid when rustix is unavailable.
    rustix::process::geteuid().as_raw()
}

fn io(op: &'static str, source: &std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("{op} failed"),
    )
    .with_source(std::io::Error::new(source.kind(), source.to_string()))
}

fn boot_error(msg: impl Into<String>) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, msg.into())
}
