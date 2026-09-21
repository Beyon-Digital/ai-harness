//! CFG-003: environment resolution freezes exact bindings at run bind
//! time; schema immutability rejects post-creation mutation.

use adapter_registry::resolver::ResolvedAdapter;
use domain::ids::{ActorId, AgentSpecId, CommandId, DaemonInstanceId, PrincipalId, RunId};
use domain::resource::WorkspaceAccessMode;
use domain::run::RunState;
use errors::codes::ErrorCode;
use kernel_store::models::NewRun;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use runtime::create_run::AgentSpecRef;
use runtime::resolved_environment::{freeze_environment, plan_environment};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;
const CONFIG: &str = include_str!("../../../config/default.yaml");

fn loop_adapter() -> ResolvedAdapter {
    ResolvedAdapter {
        adapter_id: domain::ids::AdapterId::new(&DeterministicIds::new(SEED + 99)),
        version: "0.1.0".into(),
        bundle_digest: "sha256:loop".into(),
        negotiated_capabilities: vec!["loop".into()],
        port: adapter_registry::manifest::PortImpl {
            port_id: "agent_loop".into(),
            port_version: 1,
        },
        trust_state: domain::security::TrustState::Trusted,
    }
}

fn spec_ref(ids: &DeterministicIds) -> AgentSpecRef {
    AgentSpecRef {
        agent_spec_id: AgentSpecId::new(ids),
        version: "1".into(),
        digest: "sha256:spec".into(),
    }
}

async fn open() -> (SqliteKernelStore, u64, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("k.db"),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store");
    let ids = DeterministicIds::new(SEED);
    let epoch = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence")
        .epoch
        .0;
    (store, epoch, dir)
}

async fn txn<'s>(store: &'s SqliteKernelStore, epoch: u64) -> Box<dyn KernelTxn + 's> {
    let ids = DeterministicIds::new(SEED + 1);
    store
        .begin_write(TxContext {
            daemon_epoch: epoch,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn")
}

async fn activate_gen(tx: &mut dyn KernelTxn, ids: &DeterministicIds, doc: &str) {
    use config_engine::{activate, generations, model};
    let g = generations::propose(tx, ids, doc.as_bytes().to_vec(), ActorId::new(ids), NOW)
        .await
        .expect("propose");
    generations::validate(tx, g.generation_id)
        .await
        .expect("validate");
    generations::smoke_test(tx, g.generation_id)
        .await
        .expect("smoke");
    let services = model::generation_services(&model::parse_document(doc).unwrap());
    let active = activate::active_state(tx).await.expect("state").pointer;
    let rev = active.map(|a| a.revision).unwrap_or(0);
    activate::activate(tx, g.generation_id, rev, &services, NOW)
        .await
        .expect("activate");
}

async fn insert_run(tx: &mut dyn KernelTxn, ids: &DeterministicIds) -> RunId {
    let run = RunId::new(ids);
    let task_id = domain::ids::TaskId::new(ids);
    runtime::task::ensure_task(
        tx,
        kernel_store::models::NewTask {
            task_id,
            session_id: None,
            created_by_actor_id: ActorId::new(ids),
            task_kind: "test".into(),
            payload: b"{}".to_vec(),
            created_at_ms: NOW,
        },
    )
    .await
    .expect("task");
    tx.runs()
        .insert(NewRun {
            run_id: run,
            task_id,
            session_id: None,
            parent_run_id: None,
            state: RunState::Created,
            recovery: domain::run::RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: domain::ids::EventCursor::new(
                domain::ids::EventStreamKey::new(format!("run/{run}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            created_at_ms: NOW,
        })
        .await
        .expect("run");
    run
}

#[tokio::test]
async fn run_keeps_g1_bindings_after_g2_activates() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 2);

    // G1 active -> run binds under G1.
    let mut tx = txn(&store, epoch).await;
    activate_gen(&mut *tx, &ids, CONFIG).await;
    let run_a = insert_run(&mut *tx, &ids).await;
    let plan = plan_environment(
        &mut *tx,
        &ids,
        run_a,
        "local-trusted",
        &spec_ref(&ids),
        &loop_adapter(),
        Some("ws://local/a".into()),
        Some("rev1".into()),
        WorkspaceAccessMode::ExclusiveWrite,
        b"[]".to_vec(),
        b"[]".to_vec(),
        NOW,
    )
    .await
    .expect("plan");
    let env_id = freeze_environment(&mut *tx, plan, run_a, 0)
        .await
        .expect("freeze");
    tx.commit().await.expect("commit");

    // G2 activates (run-scoped profile change only).
    let mut tx = txn(&store, epoch).await;
    let doc2 = CONFIG.replace(
        "profiles:\n  local-trusted:",
        "profiles:\n  alt:\n    agent_loop: fixture-loop\n  local-trusted:",
    );
    activate_gen(&mut *tx, &ids, &doc2).await;
    tx.commit().await.expect("commit");

    // Run A's frozen environment still references G1.
    let mut tx = txn(&store, epoch).await;
    let env = tx
        .environments()
        .get_environment(env_id)
        .await
        .expect("get")
        .expect("env");
    let g1 = tx
        .config()
        .get_active()
        .await
        .expect("active")
        .expect("ptr");
    assert_ne!(
        env.config_generation_id, g1.generation_id,
        "run A retains its G1 generation after G2 activated"
    );
    assert_eq!(env.agent_loop_id, loop_adapter().adapter_id.to_string());
    assert_eq!(env.workspace_mode, WorkspaceAccessMode::ExclusiveWrite);
}

#[tokio::test]
async fn environment_rows_are_immutable() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 3);
    let mut tx = txn(&store, epoch).await;
    activate_gen(&mut *tx, &ids, CONFIG).await;
    let run = insert_run(&mut *tx, &ids).await;
    let plan = plan_environment(
        &mut *tx,
        &ids,
        run,
        "local-trusted",
        &spec_ref(&ids),
        &loop_adapter(),
        None,
        None,
        WorkspaceAccessMode::ReadOnly,
        b"[]".to_vec(),
        b"[]".to_vec(),
        NOW,
    )
    .await
    .expect("plan");
    let env_id = freeze_environment(&mut *tx, plan, run, 0)
        .await
        .expect("freeze");
    tx.commit().await.expect("commit");

    // Direct UPDATE on the immutable table is rejected by the schema
    // trigger — surfaced as a store error, not silently applied.
    let mut tx = txn(&store, epoch).await;
    let err = tx
        .environments()
        .insert_environment(kernel_store::models::NewResolvedEnvironment {
            environment_id: env_id, // duplicate pk
            run_id: run,
            agent_spec_id: AgentSpecId::new(&ids),
            agent_spec_version: "x".into(),
            agent_spec_digest: "x".into(),
            agent_loop_id: "x".into(),
            agent_loop_version: "x".into(),
            agent_loop_digest: "x".into(),
            config_generation_id: domain::ids::ConfigGenerationId::new(&ids),
            workspace_uri: None,
            workspace_base_revision: None,
            workspace_mode: WorkspaceAccessMode::ReadOnly,
            model_provider: None,
            model_id: None,
            model_parameters: None,
            kernel_version: "x".into(),
            protocol_versions: b"[]".to_vec(),
            capability_grant_ids: b"[]".to_vec(),
            approval_request_ids: b"[]".to_vec(),
            created_at_ms: NOW,
        })
        .await
        .expect_err("environment pk conflict / immutability");
    assert!(matches!(
        err.code(),
        ErrorCode::Conflict | ErrorCode::Internal
    ));
}

#[tokio::test]
async fn registered_adapter_produces_frozen_binding_row() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 4);
    let mem_adapter = domain::ids::AdapterId::new(&DeterministicIds::new(SEED + 44));
    let mut tx = txn(&store, epoch).await;
    // Register the adapter the profile will bind.
    tx.adapters()
        .insert_registration(kernel_store::models::NewAdapterRegistration {
            adapter_id: mem_adapter,
            version: "1.0.0".into(),
            bundle_digest: "sha256:mem".into(),
            manifest_digest: "sha256:mem-manifest".into(),
            runtime_type: "process".into(),
            implemented_ports: serde_json::to_vec(&serde_json::json!([
                {"port_id": "memory_store", "port_version": 1}
            ]))
            .unwrap(),
            capabilities: serde_json::to_vec(&serde_json::json!(["memory_store.kv"])).unwrap(),
            trust_state: domain::security::TrustState::Trusted,
            conformance_state: domain::security::ConformanceState::Passed,
            created_at_ms: NOW,
        })
        .await
        .expect("register");
    let doc = CONFIG.replace(
        "profiles:\n  local-trusted:",
        &format!("profiles:\n  ext:\n    memory_store: {mem_adapter}\n  local-trusted:"),
    );
    activate_gen(&mut *tx, &ids, &doc).await;
    let run = insert_run(&mut *tx, &ids).await;
    let plan = plan_environment(
        &mut *tx,
        &ids,
        run,
        "ext",
        &spec_ref(&ids),
        &loop_adapter(),
        None,
        None,
        WorkspaceAccessMode::ReadOnly,
        b"[]".to_vec(),
        b"[]".to_vec(),
        NOW,
    )
    .await
    .expect("plan");
    assert_eq!(plan.bindings.len(), 1);
    assert_eq!(plan.bindings[0].adapter_id, mem_adapter);
    let env_id = freeze_environment(&mut *tx, plan, run, 0)
        .await
        .expect("freeze");
    tx.commit().await.expect("commit");
    let mut tx = txn(&store, epoch).await;
    let bindings = tx
        .environments()
        .get_bindings(env_id)
        .await
        .expect("bindings");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].port_id, "memory_store");
    assert_eq!(bindings[0].adapter_id, mem_adapter);
    assert_eq!(bindings[0].adapter_version, "1.0.0");
    assert_eq!(bindings[0].adapter_digest, "sha256:mem");
}

#[tokio::test]
async fn missing_registry_adapter_fails_run_start() {
    // CFG-002 smoke already fails-closed on unregistered bindings; here
    // the plan path on an active-but-mismatched generation also fails
    // before any environment row exists.
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 5);
    let mut tx = txn(&store, epoch).await;
    activate_gen(&mut *tx, &ids, CONFIG).await;
    let run = insert_run(&mut *tx, &ids).await;
    // "ghost" profile doesn't exist in the active generation.
    let err = plan_environment(
        &mut *tx,
        &ids,
        run,
        "ghost",
        &spec_ref(&ids),
        &loop_adapter(),
        None,
        None,
        WorkspaceAccessMode::ReadOnly,
        b"[]".to_vec(),
        b"[]".to_vec(),
        NOW,
    )
    .await
    .expect_err("missing profile");
    assert_eq!(err.code(), ErrorCode::NotFound);
    let row = tx.runs().get(run).await.expect("get").expect("run");
    assert!(row.resolved_environment_id.is_none());
}
