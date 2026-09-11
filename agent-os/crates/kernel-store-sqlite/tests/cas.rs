//! CAS semantics for the SQLite store.
//!
//! A matching expectation applies exactly the supplied patch and bumps the
//! revision; a stale revision, state, or cancellation epoch returns `false`
//! and mutates nothing. Constraint violations surface as stable error codes.

use std::path::Path;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, DependencyId, EventCursor, EventStreamKey, PrincipalId,
    RunId, SessionId, TaskId,
};
use domain::resource::DependencyCondition;
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{
    ClaimPatch, NewRun, NewRunDependency, NewSession, NewTask, RunCas, RunPatch,
};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::ids::DeterministicIds;

const DB_FILE: &str = "kernel.db";

fn config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections: 8,
        busy_timeout_ms: 5_000,
    }
}

fn context(provider: &DeterministicIds, daemon_epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch,
        principal_id: PrincipalId::new(provider),
        command_id: CommandId::new(provider),
        correlation_id: None,
    }
}

async fn open_store(dir: &Path) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(config(&dir.join(DB_FILE)))
        .await
        .unwrap();
    let provider = DeterministicIds::new(1_700_000_000_000);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

fn cursor(run: RunId) -> EventCursor {
    EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0)
}

struct Seeded {
    session: SessionId,
    task: TaskId,
    run: RunId,
}

async fn seed_run(store: &SqliteKernelStore, provider: &DeterministicIds, epoch: u64) -> Seeded {
    let session = SessionId::new(provider);
    let task = TaskId::new(provider);
    let run = RunId::new(provider);
    let mut txn = store.begin_write(context(provider, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: ActorId::new(provider),
            task_kind: "test".to_owned(),
            payload: vec![1, 2, 3],
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.runs()
        .insert(new_run(run, task, session))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    Seeded { session, task, run }
}

fn new_run(run: RunId, task: TaskId, session: SessionId) -> NewRun {
    NewRun {
        run_id: run,
        task_id: task,
        session_id: Some(session),
        parent_run_id: None,
        state: RunState::Created,
        recovery: RecoveryDisposition::Normal,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: cursor(run),
        cancellation_epoch: 0,
        resolved_environment_id: None,
        created_at_ms: 10,
    }
}

async fn read_run(store: &SqliteKernelStore, run: RunId) -> kernel_store::RunRow {
    let mut read = store.begin_read().await.unwrap();
    read.runs().get(run).await.unwrap().expect("run is present")
}

#[tokio::test]
async fn matching_cas_applies_patch_and_bumps_revision() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let applied = txn
        .runs()
        .cas_update(
            seeded.run,
            RunCas {
                run_revision: 0,
                state: Some(RunState::Created),
                cancellation_epoch: Some(0),
            },
            RunPatch {
                state: Some(RunState::Running),
                step_sequence: Some(1),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(applied, "matching expectation must apply");
    txn.commit().await.unwrap();

    let row = read_run(&store, seeded.run).await;
    assert_eq!(row.state, RunState::Running);
    assert_eq!(row.step_sequence, 1);
    assert_eq!(row.run_revision, 1);
}

#[tokio::test]
async fn stale_expectation_returns_false_and_mutates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert!(
        txn.runs()
            .cas_update(
                seeded.run,
                RunCas {
                    run_revision: 0,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    step_sequence: Some(7),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await
            .unwrap()
    );
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let stale_revision = txn
        .runs()
        .cas_update(
            seeded.run,
            RunCas {
                run_revision: 0,
                state: None,
                cancellation_epoch: None,
            },
            RunPatch {
                step_sequence: Some(9),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale_revision, "stale revision must not apply");

    let stale_state = txn
        .runs()
        .cas_update(
            seeded.run,
            RunCas {
                run_revision: 1,
                state: Some(RunState::Completed),
                cancellation_epoch: None,
            },
            RunPatch {
                step_sequence: Some(9),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale_state, "mismatched state must not apply");

    let stale_epoch = txn
        .runs()
        .cas_update(
            seeded.run,
            RunCas {
                run_revision: 1,
                state: None,
                cancellation_epoch: Some(5),
            },
            RunPatch {
                step_sequence: Some(9),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!stale_epoch, "mismatched cancellation epoch must not apply");

    let missing = txn
        .runs()
        .cas_update(
            RunId::new(&provider),
            RunCas {
                run_revision: 0,
                state: None,
                cancellation_epoch: None,
            },
            RunPatch {
                step_sequence: Some(9),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(!missing, "absent row must not apply");
    txn.commit().await.unwrap();

    let row = read_run(&store, seeded.run).await;
    assert_eq!(row.state, RunState::Created);
    assert_eq!(row.step_sequence, 7, "only the first CAS may mutate");
    assert_eq!(row.run_revision, 1);
}

#[tokio::test]
async fn claim_patch_is_applied_with_the_cas() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let applied = txn
        .runs()
        .cas_update(
            seeded.run,
            RunCas {
                run_revision: 0,
                state: None,
                cancellation_epoch: None,
            },
            RunPatch {
                claim: Some(ClaimPatch {
                    owner: "daemon-1".to_owned(),
                    token: 77,
                    expires_unix_ms: 1_800_000_000_000,
                    daemon_epoch: epoch,
                }),
                bump_revision: true,
                ..RunPatch::default()
            },
        )
        .await
        .unwrap();
    assert!(applied);
    txn.commit().await.unwrap();

    let row = read_run(&store, seeded.run).await;
    assert_eq!(row.claim_owner.as_deref(), Some("daemon-1"));
    assert_eq!(row.claim_token, Some(77));
    assert_eq!(row.claim_expires_ms, Some(1_800_000_000_000));
    assert_eq!(row.claim_daemon_epoch, Some(epoch));
}

#[tokio::test]
async fn unique_violation_is_conflict_and_constraint_is_failed_precondition() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let session = SessionId::new(&provider);

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(&provider),
            created_at_ms: 10,
            metadata: None,
        })
        .await
        .unwrap();
    let duplicate = txn
        .sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(&provider),
            created_at_ms: 11,
            metadata: None,
        })
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
    assert_eq!(duplicate.retry_class(), RetryClass::Never);

    let orphan = txn
        .tasks()
        .insert(NewTask {
            task_id: TaskId::new(&provider),
            session_id: Some(SessionId::new(&provider)),
            created_by_actor_id: ActorId::new(&provider),
            task_kind: "test".to_owned(),
            payload: vec![],
            created_at_ms: 10,
        })
        .await
        .unwrap_err();
    assert_eq!(orphan.code(), ErrorCode::FailedPrecondition);
    assert_eq!(orphan.retry_class(), RetryClass::Never);
    drop(txn);

    let mut read = store.begin_read().await.unwrap();
    assert!(
        read.sessions().get(session).await.unwrap().is_none(),
        "a failed transaction must leave zero rows"
    );
}

#[tokio::test]
async fn graph_head_cas_and_dependency_insertion() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;

    let source = seeded.run;
    let target = RunId::new(&provider);
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    txn.runs()
        .insert(new_run(target, seeded.task, seeded.session))
        .await
        .unwrap();
    let head = txn.graph().ensure_head(seeded.task).await.unwrap();
    assert_eq!(head.graph_revision, 0);
    let head_again = txn.graph().ensure_head(seeded.task).await.unwrap();
    assert_eq!(head_again.graph_revision, 0, "ensure_head is idempotent");
    txn.commit().await.unwrap();

    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    assert!(txn.graph().cas_head_revision(seeded.task, 0).await.unwrap());
    assert_eq!(
        txn.graph()
            .get_head(seeded.task)
            .await
            .unwrap()
            .unwrap()
            .graph_revision,
        1
    );
    assert!(
        !txn.graph().cas_head_revision(seeded.task, 0).await.unwrap(),
        "stale head revision must not advance"
    );

    let dependency = NewRunDependency {
        dependency_id: DependencyId::new(&provider),
        task_id: seeded.task,
        source_run_id: source,
        target_run_id: target,
        dependency_condition: DependencyCondition::CompletedSuccessfully,
        created_at_ms: 20,
    };
    txn.graph()
        .insert_dependency(dependency.clone(), 1)
        .await
        .unwrap();
    let head = txn.graph().get_head(seeded.task).await.unwrap().unwrap();
    assert_eq!(head.graph_revision, 2, "insert advances the head");
    let listed = txn.graph().list_dependencies(seeded.task).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].created_graph_revision, 1);
    assert!(txn.graph().is_reachable(source, target).await.unwrap());
    assert!(!txn.graph().is_reachable(target, source).await.unwrap());
    assert!(txn.graph().is_reachable(source, source).await.unwrap());

    let stale = txn
        .graph()
        .insert_dependency(
            NewRunDependency {
                dependency_id: DependencyId::new(&provider),
                ..dependency.clone()
            },
            1,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::Conflict);
    assert_eq!(stale.retry_class(), RetryClass::Never);

    let duplicate = txn
        .graph()
        .insert_dependency(
            NewRunDependency {
                dependency_id: DependencyId::new(&provider),
                ..dependency.clone()
            },
            2,
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::Conflict);
    assert_eq!(duplicate.retry_class(), RetryClass::Never);

    let orphan_task = TaskId::new(&provider);
    let missing_head = txn
        .graph()
        .insert_dependency(
            NewRunDependency {
                dependency_id: DependencyId::new(&provider),
                task_id: orphan_task,
                ..dependency
            },
            0,
        )
        .await
        .unwrap_err();
    assert_eq!(missing_head.code(), ErrorCode::NotFound);
    assert_eq!(missing_head.retry_class(), RetryClass::Never);
    drop(txn);
}

#[tokio::test]
async fn unknown_persisted_enum_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join(DB_FILE);
    let (store, epoch) = open_store(dir.path()).await;
    let provider = DeterministicIds::new(1_700_000_000_000);
    let seeded = seed_run(&store, &provider, epoch).await;
    drop(store);

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET state = 99 WHERE run_id = ?")
        .bind(seeded.run.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    drop(pool);

    let store = SqliteKernelStore::open(config(&db_path)).await.unwrap();
    let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
    let error = txn.runs().get(seeded.run).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.retry_class(), RetryClass::Never);
    drop(txn);
}
