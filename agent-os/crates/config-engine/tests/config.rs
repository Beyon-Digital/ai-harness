//! CFG-001/CFG-002: v1 parse + validation, generations pipeline,
//! CAS activation with DAEMON_RESTART_REQUIRED + rollback.

use config_engine::activate::{self, DAEMON_RESTART_REQUIRED, active_state};
use config_engine::generations::{self, testing};
use config_engine::model::{self, document_digest};
use config_engine::profile::resolve_profile;
use domain::ids::{ActorId, CommandId, ConfigGenerationId, DaemonInstanceId, PrincipalId};
use errors::codes::ErrorCode;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;
const DEFAULT_CONFIG: &str = include_str!("../../../config/default.yaml");

async fn open() -> (SqliteKernelStore, u64, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("kernel.db"),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store");
    let ids = DeterministicIds::new(SEED);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence");
    (store, fence.epoch.0, dir)
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

// ---------- CFG-001 ----------

#[test]
fn default_config_is_valid_v1() {
    let doc = model::parse_document(DEFAULT_CONFIG).expect("default parses");
    assert_eq!(doc.kernel.store, "sqlite-kernel");
    assert_eq!(doc.services.event_journal.as_deref(), Some("sqlite-events"));
    assert!(doc.profiles.contains_key("local-trusted"));
    let resolved = resolve_profile(&doc, "local-trusted").expect("profile");
    assert_eq!(
        resolved.bindings.get("agent_loop").map(String::as_str),
        Some("fixture-loop")
    );
    assert_eq!(
        resolved.bindings.get("sandbox").map(String::as_str),
        Some("local-process-t0")
    );
}

#[test]
fn version_2_rejected() {
    let doc = DEFAULT_CONFIG.replace("schema_version: 1", "schema_version: 2");
    let err = model::parse_document(&doc).expect_err("v2 rejected");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[test]
fn profile_cycle_rejected() {
    let doc = DEFAULT_CONFIG.replace(
        "profiles:\n  local-trusted:",
        "profiles:\n  a:\n    extends: b\n  b:\n    extends: a\n  local-trusted:",
    );
    let err = model::parse_document(&doc).expect_err("cycle");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
    assert!(err.message().contains("cycle"));
}

#[test]
fn missing_parent_profile_rejected() {
    let doc = DEFAULT_CONFIG.replace(
        "profiles:\n  local-trusted:",
        "profiles:\n  broken:\n    extends: ghost\n  local-trusted:",
    );
    let err = model::parse_document(&doc).expect_err("ghost parent");
    assert_eq!(err.code(), ErrorCode::InvalidArgument);
}

#[test]
fn unknown_field_in_sensitive_section_rejected() {
    for (path, injection) in [
        ("kernel", "    unexpected_key: true\n"),
        ("services", "    unexpected_key: true\n"),
        ("limits", "    surprise: 1\n"),
    ] {
        let doc =
            DEFAULT_CONFIG.replacen(&format!("{path}:\n"), &format!("{path}:\n{injection}"), 1);
        let err = model::parse_document(&doc).expect_err(&format!("unknown field under {path}"));
        assert_eq!(err.code(), ErrorCode::InvalidArgument, "{path}");
    }
}

#[test]
fn profile_extends_overrides_and_inherits() {
    let doc: config_engine::model::ConfigDocument = serde_yaml::from_str(
        r#"
schema_version: 1
kernel: {store: s}
services: {event_journal: e}
profiles:
  base: {sandbox: sbox-a, workspace: ws-a, agent_loop: loop-1}
  child: {extends: base, agent_loop: loop-2}
limits: {queue: {capacity_messages: 1}, adapters: {max_frame_bytes: 1, handshake_timeout_ms: 1, request_deadline_ms: 1, health_ping_ms: 1, health_missed_allowed: 0, supervisor_max_restarts: 0, supervisor_backoff_initial_ms: 1, supervisor_backoff_max_ms: 1}, effects: {lease_ms: 1, lease_renew_ms: 1}, runs: {claim_ttl_ms: 1}, daemon: {fence_lease_ms: 1, fence_renew_ms: 1}, approvals: {ttl_ms: 1}, shutdown: {drain_deadline_ms: 1}, events: {live_buffer_events: 1, dispatcher_poll_ms: 1, scheduler_poll_ms: 1}, streams: {read_default_limit: 1, read_max_limit: 1}, fs: {db_file_mode: "0600", runtime_dir_mode: "0700", control_socket_mode: "0600"}}
"#,
    )
    .unwrap();
    let resolved = resolve_profile(&doc, "child").expect("child resolves");
    assert_eq!(resolved.bindings["agent_loop"], "loop-2"); // override
    assert_eq!(resolved.bindings["sandbox"], "sbox-a"); // inherited
    assert_eq!(resolved.bindings["workspace"], "ws-a"); // inherited
}

#[test]
fn kernel_store_not_runtime_changeable() {
    // kernel.store is bootstrap config: it lives outside the profile map
    // entirely, so no profile key can rebind it (compile-time guarantee
    // + schema denies unknown profile fields).
    let doc = model::parse_document(DEFAULT_CONFIG).expect("doc");
    let p = resolve_profile(&doc, "local-trusted").expect("p");
    assert!(!p.bindings.contains_key("store"));
    assert!(!p.bindings.contains_key("kernel"));
}

// ---------- CFG-002 ----------

/// Propose+validate+smoke a doc; returns the generation id.
async fn pipeline(
    tx: &mut dyn KernelTxn,
    ids: &DeterministicIds,
    doc_yaml: &str,
    smoke_ok: bool,
) -> ConfigGenerationId {
    let g = generations::propose(
        tx,
        ids,
        doc_yaml.as_bytes().to_vec(),
        ActorId::new(ids),
        NOW,
    )
    .await
    .expect("propose");
    generations::validate(tx, g.generation_id)
        .await
        .expect("validate");
    if smoke_ok {
        generations::smoke_test(tx, g.generation_id)
            .await
            .expect("smoke");
    }
    g.generation_id
}

#[tokio::test]
async fn untested_generation_cannot_activate() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 2);
    let mut tx = txn(&store, epoch).await;
    // proposed but never smoke-tested (validated only).
    let g = generations::propose(
        &mut *tx,
        &ids,
        DEFAULT_CONFIG.as_bytes().to_vec(),
        ActorId::new(&ids),
        NOW,
    )
    .await
    .expect("propose");
    generations::validate(&mut *tx, g.generation_id)
        .await
        .expect("validate");
    let err = activate::activate(
        &mut *tx,
        g.generation_id,
        0,
        &model::generation_services(&model::parse_document(DEFAULT_CONFIG).unwrap()),
        NOW,
    )
    .await
    .expect_err("untested gen must not activate");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert!(
        active_state(&mut *tx)
            .await
            .expect("state")
            .pointer
            .is_none()
    );
}

#[tokio::test]
async fn activation_cas_one_winner() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 3);
    let services = model::generation_services(&model::parse_document(DEFAULT_CONFIG).unwrap());

    let mut tx = txn(&store, epoch).await;
    let g1 = pipeline(&mut *tx, &ids, DEFAULT_CONFIG, true).await;
    // Differ only in a run-scoped profile (services unchanged).
    let g2_doc = DEFAULT_CONFIG.replace(
        "profiles:\n  local-trusted:",
        "profiles:\n  alt-run:\n    agent_loop: fixture-loop\n  local-trusted:",
    );
    let g2 = pipeline(&mut *tx, &ids, &g2_doc, true).await;
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let active = activate::activate(&mut *tx, g1, 0, &services, NOW)
        .await
        .expect("activate g1");
    assert_eq!(active.generation_id, g1);
    assert_eq!(active.revision, 1);
    tx.commit().await.expect("commit");

    // Second activation with a stale revision loses the CAS race.
    let mut tx = txn(&store, epoch).await;
    let err = activate::activate(&mut *tx, g2, 0, &services, NOW)
        .await
        .expect_err("stale revision loses");
    assert_eq!(err.code(), ErrorCode::Conflict);
    // Correct revision wins.
    let active = activate::activate(&mut *tx, g2, 1, &services, NOW)
        .await
        .expect("activate g2");
    assert_eq!(active.generation_id, g2);
    assert_eq!(active.revision, 2);
}

#[tokio::test]
async fn service_binding_change_requires_restart() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 4);
    let running = model::generation_services(&model::parse_document(DEFAULT_CONFIG).unwrap());

    // G2 changes a generation-global service binding.
    let doc2 = DEFAULT_CONFIG.replace("message_queue: memory-queue", "message_queue: other-queue");
    let mut tx = txn(&store, epoch).await;
    let g1 = pipeline(&mut *tx, &ids, DEFAULT_CONFIG, true).await;
    let g2 = pipeline(&mut *tx, &ids, &doc2, true).await;
    activate::activate(&mut *tx, g1, 0, &running, NOW)
        .await
        .expect("g1");
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let err = activate::activate(&mut *tx, g2, 1, &running, NOW)
        .await
        .expect_err("restart required");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert!(err.message().contains(DAEMON_RESTART_REQUIRED));
    // Pointer unmoved.
    let active = active_state(&mut *tx)
        .await
        .expect("state")
        .pointer
        .expect("ptr");
    assert_eq!(active.generation_id, g1);
    assert_eq!(active.revision, 1);
}

#[tokio::test]
async fn failed_smoke_leaves_pointer_unchanged() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 5);
    let services = model::generation_services(&model::parse_document(DEFAULT_CONFIG).unwrap());

    let mut tx = txn(&store, epoch).await;
    let g1 = pipeline(&mut *tx, &ids, DEFAULT_CONFIG, true).await;
    activate::activate(&mut *tx, g1, 0, &services, NOW)
        .await
        .expect("g1");
    // G2 binds a missing adapter on a registry slot -> smoke fails.
    let bad = DEFAULT_CONFIG.replace(
        "agent_loop: fixture-loop",
        "agent_loop: nonexistent-adapter",
    );
    let bad_doc = {
        let g = generations::propose(
            &mut *tx,
            &ids,
            bad.as_bytes().to_vec(),
            ActorId::new(&ids),
            NOW,
        )
        .await
        .expect("propose");
        generations::validate(&mut *tx, g.generation_id)
            .await
            .expect("validate");
        let err = generations::smoke_test(&mut *tx, g.generation_id)
            .await
            .expect_err("missing adapter fails smoke");
        assert_eq!(err.code(), ErrorCode::FailedPrecondition);
        g.generation_id
    };
    let row = tx
        .config()
        .get_generation(bad_doc)
        .await
        .expect("g")
        .expect("row");
    assert_eq!(row.test_state, testing::FAILED);
    let err = activate::activate(&mut *tx, bad_doc, 1, &services, NOW)
        .await
        .expect_err("failed gen cannot activate");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    let active = active_state(&mut *tx).await.expect("s").pointer.expect("p");
    assert_eq!(active.generation_id, g1);
}

#[tokio::test]
async fn reactivation_restores_known_good() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 6);
    let services = model::generation_services(&model::parse_document(DEFAULT_CONFIG).unwrap());

    let mut tx = txn(&store, epoch).await;
    let g1 = pipeline(&mut *tx, &ids, DEFAULT_CONFIG, true).await;
    let doc2 = DEFAULT_CONFIG.replace(
        "profiles:\n  local-trusted:",
        "profiles:\n  alt-run:\n    agent_loop: fixture-loop\n  local-trusted:",
    );
    let g2 = pipeline(&mut *tx, &ids, &doc2, true).await;
    activate::activate(&mut *tx, g1, 0, &services, NOW)
        .await
        .expect("g1");
    activate::activate(&mut *tx, g2, 1, &services, NOW)
        .await
        .expect("g2");
    // Roll back to the known-good g1.
    let rolled = activate::rollback(&mut *tx, g1, 2, &services, NOW)
        .await
        .expect("rollback");
    assert_eq!(rolled.generation_id, g1);
    assert_eq!(rolled.revision, 3);
}

#[test]
fn digest_is_exact_document_bytes() {
    let d1 = document_digest(DEFAULT_CONFIG.as_bytes());
    let d2 = document_digest(format!("{DEFAULT_CONFIG} ").as_bytes());
    assert_ne!(d1, d2, "trailing byte changes identity");
}
