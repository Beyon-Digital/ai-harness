//! ART-001: atomic put/get with digest, no path leakage in agent-facing
//! results, retention-gated delete.

use artifacts::{LocalArtifactStore, PutMetadata};
use domain::ids::{
    ActorId, ArtifactId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId,
    RunId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::ErrorCode;
use kernel_store::models::{NewRun, NewTask};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;

async fn open() -> (SqliteKernelStore, u64, TempDir, LocalArtifactStore, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let art_dir = tempfile::tempdir().expect("art");
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
    (
        store,
        fence.epoch.0,
        dir,
        LocalArtifactStore::new(art_dir.path().to_path_buf()).unwrap(),
        art_dir,
    )
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

async fn run_row(txn: &mut dyn KernelTxn, run_id: RunId, ids: &DeterministicIds) {
    let task_id = TaskId::new(ids);
    txn.tasks()
        .insert(NewTask {
            task_id,
            session_id: None,
            created_by_actor_id: ActorId::new(ids),
            task_kind: "test".into(),
            payload: Vec::new(),
            created_at_ms: NOW,
        })
        .await
        .expect("task");
    txn.runs()
        .insert(NewRun {
            run_id,
            task_id,
            session_id: None,
            parent_run_id: None,
            state: RunState::Running,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 1,
            step_sequence: 0,
            input_event_cursor: EventCursor::new(
                EventStreamKey::new(format!("run/{run_id}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: String::new(),
            workspace_uri: None,
            created_at_ms: NOW,
        })
        .await
        .expect("run");
}

#[tokio::test]
async fn put_get_digest_and_metadata() {
    let (store, epoch, _d, art, _a) = open().await;
    let ids = DeterministicIds::new(SEED + 3);
    let run = RunId::new(&ids);
    let art_id = ArtifactId::new(&ids);

    let mut tx = txn(&store, epoch).await;
    run_row(&mut *tx, run, &ids).await;
    let row = art
        .put(
            &mut *tx,
            art_id,
            b"hello artifact",
            PutMetadata {
                media_type: "text/plain".into(),
                sensitivity: SensitivityClass::Internal,
                retention: RetentionClass::Standard,
                origin_effect_id: None,
            },
            run,
            NOW,
        )
        .await
        .expect("put");
    tx.commit().await.expect("commit");

    assert_eq!(row.uri, format!("artifact://{art_id}"));
    assert!(row.digest.starts_with("sha256:"));
    assert_eq!(row.size_bytes, 14);
    // The agent-facing row never exposes the physical path — the locator
    // is opaque and must not appear in the URI.
    assert!(!row.uri.contains(&row.locator));

    let mut tx = txn(&store, epoch).await;
    let (meta, data) = art.get(&mut *tx, &row.uri).await.expect("get");
    assert_eq!(data, b"hello artifact");
    assert_eq!(meta.origin_run_id, run);
    let (_, part) = art
        .get_range(&mut *tx, &row.uri, 6, 8)
        .await
        .expect("range");
    assert_eq!(part, b"artifact");
    let head = art.head(&mut *tx, &row.uri).await.expect("head");
    assert_eq!(head.digest, row.digest);
    assert_eq!(art.list_by_run(&mut *tx, run).await.expect("list").len(), 1);
}

#[tokio::test]
async fn digest_mismatch_on_read_fails_closed() {
    let (store, epoch, _d, art, art_dir) = open().await;
    let ids = DeterministicIds::new(SEED + 4);
    let run = RunId::new(&ids);
    let art_id = ArtifactId::new(&ids);

    let mut tx = txn(&store, epoch).await;
    run_row(&mut *tx, run, &ids).await;
    let row = art
        .put(
            &mut *tx,
            art_id,
            b"original",
            PutMetadata {
                media_type: "text/plain".into(),
                sensitivity: SensitivityClass::Public,
                retention: RetentionClass::Ephemeral,
                origin_effect_id: None,
            },
            run,
            NOW,
        )
        .await
        .expect("put");
    tx.commit().await.expect("commit");

    // Bit-rot: swap file contents without touching the row.
    std::fs::write(art_dir.path().join(&row.locator), b"tampered").expect("tamper");
    let mut tx = txn(&store, epoch).await;
    let err = art
        .get(&mut *tx, &row.uri)
        .await
        .expect_err("digest mismatch");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn retention_gates_delete() {
    let (store, epoch, _d, art, art_dir) = open().await;
    let ids = DeterministicIds::new(SEED + 5);
    let run = RunId::new(&ids);

    let mut tx = txn(&store, epoch).await;
    run_row(&mut *tx, run, &ids).await;
    let audit = art
        .put(
            &mut *tx,
            ArtifactId::new(&ids),
            b"keep me",
            PutMetadata {
                media_type: "text/plain".into(),
                sensitivity: SensitivityClass::Confidential,
                retention: RetentionClass::Audit,
                origin_effect_id: None,
            },
            run,
            NOW,
        )
        .await
        .expect("audit put");
    let eph = art
        .put(
            &mut *tx,
            ArtifactId::new(&ids),
            b"drop me",
            PutMetadata {
                media_type: "text/plain".into(),
                sensitivity: SensitivityClass::Public,
                retention: RetentionClass::Ephemeral,
                origin_effect_id: None,
            },
            run,
            NOW,
        )
        .await
        .expect("eph put");
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let err = art
        .delete(&mut *tx, &audit.uri)
        .await
        .expect_err("audit is pinned");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    art.delete(&mut *tx, &eph.uri)
        .await
        .expect("ephemeral deletable");
    assert!(!art_dir.path().join(&eph.locator).exists());
}
