//! Property tests over generated workloads: one CAS winner (P2), contiguous
//! concurrent stream sequences (P3), and replay invariance (P4).
//!
//! Each case runs against a fresh temporary store on a real multi-threaded
//! runtime. Concurrency is synchronized with barriers, never with sleeps (N2).

use std::path::Path;
use std::sync::{Arc, Mutex};

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventId, EventStreamKey, IdempotencyKey,
    PrincipalId, RunId, SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use kernel_store::models::{
    NewIdempotencyRecord, NewOutboxEvent, NewRun, NewSession, NewTask, RunCas, RunPatch,
};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use proptest::prelude::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const DB_FILE: &str = "kernel.db";
const SEED_MS: i64 = 1_700_000_000_000;
const PROP_CASES: u32 = 16;

/// Every table created by the inception schema; a replay must leave all of
/// them exactly as it found them (P4).
const CANONICAL_TABLES: [&str; 30] = [
    "kernel_meta",
    "daemon_fence",
    "agent_specs",
    "sessions",
    "tasks",
    "resolved_run_environments",
    "runs",
    "resolved_bindings",
    "run_graph_heads",
    "run_dependencies",
    "workspace_leases",
    "effects",
    "resource_reservations",
    "timers",
    "capability_grants",
    "delegation_hops",
    "approval_requests",
    "approval_responses",
    "adapter_registrations",
    "config_generations",
    "active_config_generation",
    "idempotency_records",
    "event_stream_heads",
    "outbox_events",
    "artifacts",
    "workspaces",
    "loop_turns",
    "decisions",
    "adapter_instances",
    "conformance_reports",
];

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("property test runtime")
}

fn config(db_path: &Path, pool_max_connections: u32) -> StoreConfig {
    StoreConfig {
        path: db_path.to_path_buf(),
        pool_max_connections,
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

async fn open_store(dir: &Path, pool_max_connections: u32) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(config(&dir.join(DB_FILE), pool_max_connections))
        .await
        .unwrap();
    let provider = DeterministicIds::new(SEED_MS);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

fn cursor(run: RunId) -> EventCursor {
    let stream = match EventStreamKey::new(format!("run/{run}")) {
        Ok(stream) => stream,
        Err(error) => panic!("stream key rejected: {error}"),
    };
    EventCursor::new(stream, 0)
}

async fn seed_run(store: &SqliteKernelStore, provider: &DeterministicIds, epoch: u64) -> RunId {
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
            task_kind: "property".to_owned(),
            payload: vec![],
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.runs()
        .insert(NewRun {
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
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    run
}

async fn canonical_counts(db_path: &Path) -> Vec<i64> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut counts = Vec::with_capacity(CANONICAL_TABLES.len());
    for table in CANONICAL_TABLES {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        counts.push(count);
    }
    counts
}

fn staged_event(
    provider: &DeterministicIds,
    stream_key: &EventStreamKey,
    sequence: u64,
    task: usize,
) -> NewOutboxEvent {
    NewOutboxEvent {
        event_id: EventId::new(provider),
        event_type: format!("property.{task}"),
        event_version: 1,
        stream_key: stream_key.clone(),
        sequence,
        occurred_at_ms: SEED_MS,
        run_id: None,
        task_id: None,
        session_id: None,
        effect_id: None,
        causation_id: None,
        correlation_id: None,
        sensitivity: SensitivityClass::Internal,
        retention: RetentionClass::Audit,
        payload: vec![task as u8],
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(PROP_CASES))]

    /// P2: for any set of writers racing one CAS expectation, exactly one
    /// commits and the persisted row reflects only that writer's patch.
    #[test]
    fn cas_race_has_exactly_one_winner(writers in 2usize..=6, step in 1u64..=10_000) {
        runtime().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let (store, epoch) = open_store(dir.path(), writers as u32 + 2).await;
            let store = Arc::new(store);
            let provider = DeterministicIds::new(SEED_MS);
            let run = seed_run(&store, &provider, epoch).await;
            let barrier = Arc::new(Barrier::new(writers));

            let mut handles = Vec::new();
            for _ in 0..writers {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                let ctx = context(&provider, epoch);
                handles.push(tokio::spawn(async move {
                    barrier.wait().await;
                    let mut txn = store.begin_write(ctx).await.unwrap();
                    let won = txn
                        .runs()
                        .cas_update(
                            run,
                            RunCas {
                                run_revision: 0,
                                state: None,
                                cancellation_epoch: None,
                            },
                            RunPatch {
                                step_sequence: Some(step),
                                bump_revision: true,
                                ..RunPatch::default()
                            },
                        )
                        .await
                        .unwrap();
                    txn.commit().await.unwrap();
                    won
                }));
            }

            let mut wins = 0usize;
            for handle in handles {
                if handle.await.unwrap() {
                    wins += 1;
                }
            }
            assert_eq!(wins, 1, "exactly one of {writers} racing writers may commit");

            let mut read = store.begin_read().await.unwrap();
            let row = read.runs().get(run).await.unwrap().unwrap();
            assert_eq!(row.run_revision, 1);
            assert_eq!(row.step_sequence, step);
        });
    }

    /// P3: for any interleaving of concurrent appends to one stream, the
    /// allocated sequences are unique and contiguous from one.
    #[test]
    fn concurrent_appends_are_contiguous(
        tasks in 2usize..=4,
        burst in 1usize..=5,
        txn_size in 1usize..=3,
    ) {
        runtime().block_on(async {
            let total = tasks * burst;
            let dir = tempfile::tempdir().unwrap();
            let (store, epoch) = open_store(dir.path(), tasks as u32 + 2).await;
            let store = Arc::new(store);
            let provider = Arc::new(DeterministicIds::new(SEED_MS));
            let stream_key = match EventStreamKey::new("run/property-appends") {
                Ok(stream) => stream,
                Err(error) => panic!("stream key rejected: {error}"),
            };
            let barrier = Arc::new(Barrier::new(tasks));
            let staged = Arc::new(Mutex::new(Vec::with_capacity(total)));

            let mut handles = Vec::new();
            for task in 0..tasks {
                let store = Arc::clone(&store);
                let provider = Arc::clone(&provider);
                let barrier = Arc::clone(&barrier);
                let staged = Arc::clone(&staged);
                let stream_key = stream_key.clone();
                handles.push(tokio::spawn(async move {
                    let mut remaining = burst;
                    barrier.wait().await;
                    while remaining > 0 {
                        let chunk = txn_size.min(remaining);
                        let mut txn = store
                            .begin_write(context(provider.as_ref(), epoch))
                            .await
                            .unwrap();
                        for _ in 0..chunk {
                            let sequence =
                                txn.streams().allocate(stream_key.clone()).await.unwrap();
                            txn.streams()
                                .insert_outbox(staged_event(
                                    provider.as_ref(),
                                    &stream_key,
                                    sequence,
                                    task,
                                ))
                                .await
                                .unwrap();
                            staged.lock().unwrap().push(sequence);
                        }
                        txn.commit().await.unwrap();
                        remaining -= chunk;
                    }
                }));
            }
            for handle in handles {
                handle.await.unwrap();
            }

            let mut sequences = staged.lock().unwrap().clone();
            sequences.sort_unstable();
            let expected: Vec<u64> = (1..=total as u64).collect();
            assert_eq!(
                sequences, expected,
                "allocations must cover 1..={total} exactly once"
            );

            let mut txn = store
                .begin_write(context(provider.as_ref(), epoch))
                .await
                .unwrap();
            let rows = txn
                .streams()
                .scan_unpublished(total as u32)
                .await
                .unwrap();
            let scanned: Vec<u64> = rows.iter().map(|row| row.sequence).collect();
            assert_eq!(scanned, expected, "the ordered scan must match allocation order");
            txn.rollback().await.unwrap();
        });
    }

    /// P4: for any stored outcome and any number of replays with the same
    /// principal, key, and digest, the stored outcome is returned and every
    /// canonical table keeps its row count.
    #[test]
    fn replay_returns_the_stored_outcome_unchanged(
        outcome_code in "[a-z][a-z0-9_]{0,15}",
        payload in prop::collection::vec(any::<u8>(), 0..16),
        replays in 1usize..=5,
    ) {
        runtime().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let (store, epoch) = open_store(dir.path(), 4).await;
            let provider = DeterministicIds::new(SEED_MS);
            let principal = PrincipalId::new(&provider);
            let idempotency_key = IdempotencyKey::new("property-replay").unwrap();
            let digest = "property-digest";

            let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
            txn.idempotency()
                .insert(NewIdempotencyRecord {
                    principal_id: principal,
                    idempotency_key: idempotency_key.clone(),
                    request_digest: digest.to_owned(),
                    command_id: CommandId::new(&provider),
                    outcome_code: outcome_code.clone(),
                    outcome_payload: payload.clone(),
                    created_at_ms: SEED_MS,
                })
                .await
                .unwrap();
            txn.commit().await.unwrap();

            let db_path = dir.path().join(DB_FILE);
            let before = canonical_counts(&db_path).await;

            for _ in 0..replays {
                let mut txn = store.begin_write(context(&provider, epoch)).await.unwrap();
                let stored = txn
                    .idempotency()
                    .lookup(principal, &idempotency_key)
                    .await
                    .unwrap()
                    .expect("a replay finds the stored record");
                assert_eq!(stored.request_digest, digest);
                assert_eq!(stored.outcome_code, outcome_code);
                assert_eq!(stored.outcome_payload, payload);
                assert_eq!(stored.created_at_ms, SEED_MS);
                txn.commit().await.unwrap();
            }

            let after = canonical_counts(&db_path).await;
            assert_eq!(
                after, before,
                "a replay must not change any canonical row count"
            );
            assert_eq!(
                after[CANONICAL_TABLES
                    .iter()
                    .position(|table| *table == "idempotency_records")
                    .expect("idempotency_records is a canonical table")],
                1
            );
        });
    }
}
