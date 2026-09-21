//! INT-004 concurrency suite — races every kernel linearization point the
//! MVP relies on, against the real sqlite store, barrier-synced with no
//! timing sleeps. Single-winner invariants only; the seeded property suite
//! in `tests/property` covers the state-space claims.
#![cfg(unix)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::ids::{
    AdapterId, CommandId, DaemonInstanceId, DecisionId, DependencyId, EffectId, EventCursor,
    EventStreamKey, LeaseId, PrincipalId, ReservationId, RunId, SessionId, TaskId, TimerId,
    WorkspaceId,
};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use kernel_store::models::{
    NewEffect, NewRun, NewRunDependency, NewSession, NewTask, NewTimer, NewWorkspaceLease,
};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use std::str::FromStr;
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;
use tokio::sync::Barrier;

const RACERS: usize = 8;
const ROUNDS: usize = 64;
const SEED: i64 = 1_700_400_000_000;
const NOW: i64 = 1_700_400_000_000;

fn ctx(ids: &DeterministicIds, epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch: epoch,
        principal_id: PrincipalId::new(ids),
        command_id: CommandId::new(ids),
        correlation_id: None,
    }
}

async fn open(tag: &str) -> (tempfile::TempDir, Arc<SqliteKernelStore>, u64) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteKernelStore::open(StoreConfig {
            path: dir.path().join(format!("{tag}.db")),
            pool_max_connections: RACERS as u32 + 2,
            busy_timeout_ms: 5_000,
        })
        .await
        .unwrap(),
    );
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&DeterministicIds::new(SEED - 1)))
        .await
        .unwrap();
    (dir, store, fence.epoch.0)
}

async fn seed_session_task(
    store: &SqliteKernelStore,
    ids: &DeterministicIds,
    epoch: u64,
) -> TaskId {
    let session = SessionId::new(ids);
    let task = TaskId::new(ids);
    let mut txn = store.begin_write(ctx(ids, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: PrincipalId::new(ids),
            created_at_ms: NOW,
            metadata: None,
        })
        .await
        .unwrap();
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: domain::ids::ActorId::new(ids),
            task_kind: "agent".to_owned(),
            payload: Vec::new(),
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    task
}

async fn insert_run(
    store: &SqliteKernelStore,
    ids: &DeterministicIds,
    epoch: u64,
    task: TaskId,
    state: RunState,
) -> RunId {
    let run = RunId::new(ids);
    let mut txn = store.begin_write(ctx(ids, epoch)).await.unwrap();
    txn.runs()
        .insert(NewRun {
            run_id: run,
            task_id: task,
            session_id: None,
            parent_run_id: None,
            state,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: EventCursor::new(
                EventStreamKey::new(format!("run/{run}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: "local-trusted".to_owned(),
            workspace_uri: None,
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    run
}

/// Exit C7: racing ready-run claims serialize — exactly one winner, one
/// committed revision bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_ready_run_claims_single_winner() {
    let (_d, store, epoch) = open("claim").await;
    let ids = DeterministicIds::new(SEED + 1);
    let task = seed_session_task(&store, &ids, epoch).await;
    let run = insert_run(&store, &ids, epoch, task, RunState::Ready).await;

    let barrier = Arc::new(Barrier::new(RACERS));
    let mut handles = Vec::new();
    for i in 0..RACERS {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let ids = DeterministicIds::new(SEED + 100 + i as i64);
            barrier.wait().await;
            let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
            let outcome = runtime::claim::claim_with_ids(
                &mut *txn,
                run,
                format!("claimant-{i}"),
                30_000,
                NOW,
                epoch,
                &ids,
            )
            .await;
            match outcome {
                Ok(()) => {
                    txn.commit().await.unwrap();
                    true
                }
                Err(_) => {
                    txn.rollback().await.ok();
                    false
                }
            }
        }));
    }
    let mut wins = 0;
    for h in handles {
        if h.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "ready-run claim must be single-winner");

    let mut read = store.begin_read().await.unwrap();
    let row = read.runs().get(run).await.unwrap().unwrap();
    assert_eq!(row.state, RunState::Running);
    assert_eq!(row.run_revision, 1);
}

/// Exit C6: concurrent edge insertions can never produce a cycle — the
/// reachability check and the insert share one immediate transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_graph_edge_insertions_never_cycle() {
    let (_d, store, epoch) = open("graph").await;
    let ids = DeterministicIds::new(SEED + 2);
    let task = seed_session_task(&store, &ids, epoch).await;
    let mut runs = Vec::new();
    for _ in 0..16 {
        runs.push(insert_run(&store, &ids, epoch, task, RunState::Running).await);
    }
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        txn.graph().ensure_head(task).await.unwrap();
        txn.commit().await.unwrap();
    }

    let attempts = Arc::new(AtomicU64::new(0));
    let committed = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(RACERS));
    let mut handles = Vec::new();
    for lane in 0..RACERS {
        let store = store.clone();
        let barrier = barrier.clone();
        let attempts = attempts.clone();
        let committed = committed.clone();
        let runs = runs.clone();
        handles.push(tokio::spawn(async move {
            let ids = DeterministicIds::new(SEED + 200 + lane as i64);
            barrier.wait().await;
            for round in 0..ROUNDS {
                // Alternate directions so cycle-shaped pairs get attempted:
                // a committed A->B means a later B->A must be rejected.
                let a = runs[(lane + round) % runs.len()];
                let b = runs[(lane + round + 1 + (round % 3)) % runs.len()];
                if a == b {
                    continue;
                }
                let (source, target) = if round % 2 == 0 { (a, b) } else { (b, a) };
                let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
                let head = txn.graph().ensure_head(task).await.unwrap();
                let outcome = txn
                    .graph()
                    .insert_dependency(
                        NewRunDependency {
                            dependency_id: DependencyId::new(&ids),
                            task_id: task,
                            source_run_id: source,
                            target_run_id: target,
                            dependency_condition: DependencyCondition::AnyTerminal,
                            created_at_ms: NOW,
                        },
                        head.graph_revision,
                    )
                    .await;
                attempts.fetch_add(1, Ordering::SeqCst);
                match outcome {
                    Ok(()) => {
                        txn.commit().await.unwrap();
                        committed.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(_) => {
                        txn.rollback().await.ok();
                    }
                }
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert!(attempts.load(Ordering::SeqCst) > 0);

    // Final check over the committed edge set: no edge may close a cycle.
    let mut read = store.begin_read().await.unwrap();
    let edges: Vec<(RunId, RunId)> = read
        .graph()
        .list_dependencies(task)
        .await
        .unwrap()
        .iter()
        .map(|d| (d.source_run_id, d.target_run_id))
        .collect();
    let mut adj: HashMap<RunId, Vec<RunId>> = HashMap::new();
    for (s, t) in &edges {
        adj.entry(*s).or_default().push(*t);
    }
    fn reaches(adj: &HashMap<RunId, Vec<RunId>>, from: RunId, to: RunId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if seen.insert(n) {
                stack.extend(adj.get(&n).cloned().unwrap_or_default());
            }
        }
        false
    }
    for (s, t) in &edges {
        assert!(
            !reaches(&adj, *t, *s),
            "cycle closed: {s} -> {t} coexists with a return path"
        );
    }
}

/// Exit C12: timer fire vs cancel from the same version — one linearized
/// winner, and the persisted row is exactly that winner's state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_timer_fire_cancel_single_winner() {
    let (_d, store, epoch) = open("timer").await;
    let ids = DeterministicIds::new(SEED + 3);
    let task = seed_session_task(&store, &ids, epoch).await;
    let run = insert_run(&store, &ids, epoch, task, RunState::Running).await;

    for round in 0..32 {
        let timer = TimerId::new(&ids);
        {
            let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
            txn.timers()
                .insert(NewTimer {
                    timer_id: timer,
                    run_id: Some(run),
                    timer_kind: "agentos.internal.RunWaitExpired".to_owned(),
                    payload: Vec::new(),
                    due_at_ms: NOW,
                    state: TimerState::Scheduled,
                    version: 0,
                    created_at_ms: NOW,
                })
                .await
                .unwrap();
            txn.commit().await.unwrap();
        }
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for target in [TimerState::Fired, TimerState::Cancelled] {
            let store = store.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let mut txn = store
                    .begin_write(ctx(&DeterministicIds::new(SEED + 500 + round), epoch))
                    .await
                    .unwrap();
                let won = txn
                    .timers()
                    .cas_transition(
                        timer,
                        TimerState::Scheduled,
                        0,
                        kernel_store::models::TimerPatch {
                            state: Some(target),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                if won {
                    txn.commit().await.unwrap();
                } else {
                    txn.rollback().await.ok();
                }
                (won, target)
            }));
        }
        let mut winners = Vec::new();
        for h in handles {
            let (won, target) = h.await.unwrap();
            if won {
                winners.push(target);
            }
        }
        assert_eq!(winners.len(), 1, "round {round}: exactly one winner");
        let mut read = store.begin_read().await.unwrap();
        let row = read.timers().get(timer).await.unwrap().unwrap();
        assert_eq!(row.state, winners[0]);
        assert_eq!(row.version, 1);
    }
}

/// Exit C10: racing effect claims produce exactly one fencing token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_effect_claim_single_fencer() {
    let (_d, store, epoch) = open("effect-claim").await;
    let ids = DeterministicIds::new(SEED + 4);
    let task = seed_session_task(&store, &ids, epoch).await;
    let run = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let effect = seed_prepared_effect(&store, &ids, epoch, run).await;

    let barrier = Arc::new(Barrier::new(RACERS));
    let mut handles = Vec::new();
    for i in 0..RACERS {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let ids = DeterministicIds::new(SEED + 600 + i as i64);
            barrier.wait().await;
            let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
            let token = txn
                .effects()
                .claim(effect, &format!("executor-{i}"), epoch, NOW + 30_000, NOW)
                .await
                .unwrap();
            txn.commit().await.unwrap();
            token
        }));
    }
    let mut tokens = Vec::new();
    for h in handles {
        if let Some(token) = h.await.unwrap() {
            tokens.push(token);
        }
    }
    assert_eq!(tokens.len(), 1, "exactly one racing claim may win");
    assert_eq!(tokens[0], 1);
}

/// Exit C10 (second half): a committed effect never accepts a second
/// executor's transition under a different fencing token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_effect_commit_single_fencer() {
    let (_d, store, epoch) = open("effect-commit").await;
    let ids = DeterministicIds::new(SEED + 5);
    let task = seed_session_task(&store, &ids, epoch).await;
    let run = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let effect = seed_prepared_effect(&store, &ids, epoch, run).await;

    let token = {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let token = txn
            .effects()
            .claim(effect, "executor-real", epoch, NOW + 30_000, NOW)
            .await
            .unwrap()
            .unwrap();
        txn.commit().await.unwrap();
        token
    };
    assert_eq!(token, 1);

    // "real" (executor-real, token 1) races "forged" (executor-stale, token
    // 2): only the recorded fencer may move the row.
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for (label, executor, tok) in [
        ("real", "executor-real", token),
        ("forged", "executor-stale", token + 1),
    ] {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let ids = DeterministicIds::new(SEED + 700);
            let clock = TestClock::new(NOW);
            let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
            let env = effects::EffectEnv {
                ids: &ids,
                clock: &clock,
                correlation_id: None,
                causation_id: None,
            };
            let executor = effects::ExecutorRef {
                executor_id: executor,
                fencing_token: tok,
            };
            let ok = async {
                effects::mark_dispatched(&mut *txn, &env, effect, &executor, None).await?;
                effects::acknowledge(
                    &mut *txn,
                    &env,
                    effect,
                    &executor,
                    Some("fixture://counter/1".to_owned()),
                )
                .await?;
                effects::commit(
                    &mut *txn,
                    &env,
                    effect,
                    &executor,
                    Some("fixture://counter/1".to_owned()),
                )
                .await
            }
            .await;
            match ok {
                Ok(_) => {
                    txn.commit().await.unwrap();
                    label
                }
                Err(_) => {
                    txn.rollback().await.ok();
                    label
                }
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut read = store.begin_read().await.unwrap();
    let row = read.effects().get(effect).await.unwrap().unwrap();
    assert_eq!(
        row.state,
        domain::effect::EffectState::Committed,
        "the real fencer commits; the forged token can only fail"
    );
    assert_eq!(row.executor_id.as_deref(), Some("executor-real"));
}

/// Exit C17: two racing exclusive-write transfers at the same lease epoch —
/// exactly one commits; the loser's token is dead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_exclusive_transfer_single_winner() {
    let (_d, store, epoch) = open("lease").await;
    let ids = DeterministicIds::new(SEED + 6);
    let task = seed_session_task(&store, &ids, epoch).await;
    let owner_a = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let owner_b = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let owner_c = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let workspace = WorkspaceId::new(&ids);
    let lease_id = LeaseId::new(&ids);
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        txn.workspaces()
            .insert_lease(NewWorkspaceLease {
                lease_id,
                workspace_id: workspace,
                owner_run_id: owner_a,
                mode: WorkspaceAccessMode::ExclusiveWrite,
                lease_epoch: 1,
                enforcement_state: LeaseEnforcementState::Active,
                delegated_from: Vec::new(),
                created_at_ms: NOW,
            })
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for target in [owner_b, owner_c] {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let ids = DeterministicIds::new(SEED + 800);
            barrier.wait().await;
            let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
            let outcome = workspace_crate::coordinator::transfer_exclusive(
                &mut *txn,
                lease_id,
                owner_a,
                target,
                1,
                Vec::new(),
            )
            .await;
            match outcome {
                Ok(_) => {
                    txn.commit().await.unwrap();
                    true
                }
                Err(_) => {
                    txn.rollback().await.ok();
                    false
                }
            }
        }));
    }
    let mut wins = 0;
    for h in handles {
        if h.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "exactly one transfer at epoch 1 may commit");
    let mut read = store.begin_read().await.unwrap();
    let row = read
        .workspaces()
        .get_lease(lease_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.lease_epoch, 2);
    assert!(row.owner_run_id == owner_b || row.owner_run_id == owner_c);
}

/// Exit C13 under contention: children racing to reserve under the same
/// parent can never push the delegated sum past the parent budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raced_reservation_children_never_exceed_parent_budget() {
    let (_d, store, epoch) = open("budget").await;
    let ids = DeterministicIds::new(SEED + 7);
    let task = seed_session_task(&store, &ids, epoch).await;
    let run = insert_run(&store, &ids, epoch, task, RunState::Running).await;
    let parent = ReservationId::new(&ids);
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let clock = TestClock::new(NOW);
        let env = resources::ResourceEnv {
            ids: &ids,
            clock: &clock,
            correlation_id: None,
            causation_id: None,
        };
        resources::reserve(
            &mut *txn,
            &env,
            resources::ReserveRequest {
                reservation_id: Some(parent),
                run_id: run,
                unit: resources::ResourceUnit::WallClockMs,
                amount: 1_000,
                parent: None,
                fencing_token: 0,
            },
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }

    let barrier = Arc::new(Barrier::new(RACERS));
    let mut handles = Vec::new();
    for i in 0..RACERS {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let ids = DeterministicIds::new(SEED + 900 + i as i64);
            barrier.wait().await;
            let mut granted = 0i64;
            for _ in 0..8 {
                // Each racer asks for 200 of the 1000-unit parent budget.
                let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
                let clock = TestClock::new(NOW);
                let env = resources::ResourceEnv {
                    ids: &ids,
                    clock: &clock,
                    correlation_id: None,
                    causation_id: None,
                };
                let outcome = resources::reserve(
                    &mut *txn,
                    &env,
                    resources::ReserveRequest {
                        reservation_id: None,
                        run_id: run,
                        unit: resources::ResourceUnit::WallClockMs,
                        amount: 200,
                        parent: Some(parent),
                        fencing_token: 0,
                    },
                )
                .await;
                match outcome {
                    Ok(_) => {
                        txn.commit().await.unwrap();
                        granted += 200;
                    }
                    Err(e) => {
                        assert_eq!(
                            e.code(),
                            errors::codes::ErrorCode::ResourceExhausted,
                            "over-budget must fail ResourceExhausted, got {e}"
                        );
                        txn.rollback().await.ok();
                    }
                }
            }
            granted
        }));
    }
    let mut total = 0;
    for h in handles {
        total += h.await.unwrap();
    }
    assert_eq!(
        total, 1_000,
        "children may only ever delegate the parent's budget"
    );
    let mut read = store.begin_read().await.unwrap();
    let children = read.resources().list_children(parent).await.unwrap();
    let sum: i64 = children
        .iter()
        .filter(|c| c.state == ReservationState::Reserved)
        .map(|c| c.amount)
        .sum();
    assert_eq!(sum, 1_000);
}

async fn seed_prepared_effect(
    store: &SqliteKernelStore,
    ids: &DeterministicIds,
    epoch: u64,
    run: RunId,
) -> EffectId {
    let effect = EffectId::new(ids);
    let mut txn = store.begin_write(ctx(ids, epoch)).await.unwrap();
    txn.effects()
        .insert(NewEffect {
            effect_id: effect,
            run_id: run,
            step_sequence: 1,
            decision_id: DecisionId::new(ids),
            operation: "fixture.increment_counter".to_owned(),
            request_hash: "ab".repeat(32),
            request_payload: Vec::new(),
            effect_class: domain::effect::EffectClass::ExternalMutation,
            idempotency_semantics: domain::effect::IdempotencySemantics::IdempotencyKeySupported,
            reconciliation_semantics: domain::effect::ReconciliationSemantics::StatusLookup,
            cancellation_semantics: "best_effort".to_owned(),
            compensation_capability: None,
            adapter_id: AdapterId::from_str("01905c5e-0000-7000-8000-e11ec7ad01ef").unwrap(),
            adapter_version: "0.1.0".to_owned(),
            adapter_digest: "sha256:effect".to_owned(),
            state: domain::effect::EffectState::Prepared,
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    effect
}
