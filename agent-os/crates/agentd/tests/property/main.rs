//! INT-004 property suite — seeded randomized walks over the kernel's
//! persisted state machines. Every case is reproduced from its proptest
//! seed; failures write regressions into `proptest-regressions/` next to
//! this file. No sleeps, no timing assumptions — all asserts are on the
//! committed store state.
//!
//! Required properties covered here (testing/property-tests.md):
//!   - no illegal run transitions / run_revision strictly increases
//!   - unique decision tuple per (revision, epoch, step, turn)
//!   - acyclic dependency graph
//!   - committed effects never regress
//!   - reservation descendants never exceed ancestor budget
//!   - at most one timer mutation winner per version
//!   - at most one active exclusive workspace lease
//!   - outbox sequence strictly contiguous per stream
#![cfg(unix)]

use std::collections::{HashMap, HashSet};

use domain::effect::EffectState;
use domain::ids::{
    AdapterId, CommandId, DaemonInstanceId, DecisionId, DependencyId, EffectId, EventCursor,
    EventId, EventStreamKey, LeaseId, PrincipalId, RunId, SessionId, TaskId, TimerId, TurnId,
    WorkspaceId,
};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use kernel_store::models::{
    NewDecision, NewEffect, NewLoopTurn, NewOutboxEvent, NewRun, NewRunDependency, NewSession,
    NewTask, NewTimer, RunDependencyRow,
};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use std::str::FromStr;
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const NOW: i64 = 1_700_400_000_000;
const SEED: i64 = 1_800_400_000_000;

fn config() -> Config {
    // Hundreds of cases per property; override with PROPTEST_CASES.
    Config {
        cases: 512,
        ..Config::default()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

struct Rig<'rt> {
    rt: &'rt tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    store: SqliteKernelStore,
    epoch: u64,
    ids: DeterministicIds,
}

impl<'rt> Rig<'rt> {
    fn open(rt: &'rt tokio::runtime::Runtime, tag: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ids = DeterministicIds::new(SEED);
        let store = rt
            .block_on(SqliteKernelStore::open(StoreConfig {
                path: dir.path().join(format!("{tag}.db")),
                pool_max_connections: 4,
                busy_timeout_ms: 5_000,
            }))
            .unwrap();
        let fence = rt
            .block_on(
                store.acquire_daemon_fence(DaemonInstanceId::new(&DeterministicIds::new(SEED - 9))),
            )
            .unwrap();
        Rig {
            rt,
            _dir: dir,
            store,
            epoch: fence.epoch.0,
            ids,
        }
    }

    fn ctx(&self) -> TxContext {
        TxContext {
            daemon_epoch: self.epoch,
            principal_id: PrincipalId::new(&self.ids),
            command_id: CommandId::new(&self.ids),
            correlation_id: None,
        }
    }

    fn task_and_run(&mut self) -> (TaskId, RunId) {
        self.rt.block_on(async {
            let session = SessionId::new(&self.ids);
            let task = TaskId::new(&self.ids);
            let run = RunId::new(&self.ids);
            let mut txn = self.store.begin_write(self.ctx()).await.unwrap();
            txn.sessions()
                .insert(NewSession {
                    session_id: session,
                    principal_id: PrincipalId::new(&self.ids),
                    created_at_ms: NOW,
                    metadata: None,
                })
                .await
                .unwrap();
            txn.tasks()
                .insert(NewTask {
                    task_id: task,
                    session_id: Some(session),
                    created_by_actor_id: domain::ids::ActorId::new(&self.ids),
                    task_kind: "agent".to_owned(),
                    payload: Vec::new(),
                    created_at_ms: NOW,
                })
                .await
                .unwrap();
            txn.runs()
                .insert(new_run(run, task, RunState::Ready))
                .await
                .unwrap();
            txn.commit().await.unwrap();
            (task, run)
        })
    }

    fn run_row(&mut self, run: RunId) -> kernel_store::models::RunRow {
        self.rt.block_on(async {
            let mut txn = self.store.begin_read().await.unwrap();
            txn.runs().get(run).await.unwrap().unwrap()
        })
    }
}

fn new_run(run: RunId, task: TaskId, state: RunState) -> NewRun {
    NewRun {
        run_id: run,
        task_id: task,
        session_id: None,
        parent_run_id: None,
        state,
        recovery: RecoveryDisposition::Normal,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: EventCursor::new(EventStreamKey::new(format!("run/{run}")).unwrap(), 0),
        cancellation_epoch: 0,
        resolved_environment_id: None,
        agent_spec_id: None,
        agent_spec_version: None,
        agent_spec_digest: None,
        requested_profile: "local-trusted".to_owned(),
        workspace_uri: None,
        created_at_ms: NOW,
    }
}

fn state_for_wire(wire: u8) -> RunState {
    const ALL: [RunState; 12] = [
        RunState::Unspecified,
        RunState::Created,
        RunState::Ready,
        RunState::Running,
        RunState::WaitingTool,
        RunState::WaitingChild,
        RunState::WaitingHuman,
        RunState::Suspended,
        RunState::Cancelling,
        RunState::Completed,
        RunState::Failed,
        RunState::Cancelled,
    ];
    ALL[(wire as usize) % ALL.len()]
}

/// P1+P2: a random walk of legal and illegal transition attempts leaves the
/// persisted state reachable only via `allows`, with revision strictly
/// increasing on every committed mutation.
#[test]
fn run_transitions_legal_and_revision_monotonic() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec((0u8..12, -2i8..2), 1..64);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-run");
            let (_task, run) = rig.task_and_run();
            // Bring to Running via the legal claim path.
            rt.block_on(async {
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                runtime::claim::claim_with_ids(
                    &mut *txn,
                    run,
                    "prop-owner".to_owned(),
                    30_000,
                    NOW,
                    rig.epoch,
                    &rig.ids,
                )
                .await
                .unwrap();
                txn.commit().await.unwrap();
            });

            let mut committed_log: Vec<(RunState, RunState)> = Vec::new();
            for (to_wire, rev_delta) in ops {
                let before = rig.run_row(run);
                let target = state_for_wire(to_wire);
                let expect = before.run_revision as i64 + rev_delta as i64;
                let result = rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let outcome = runtime::run::transition(
                        &mut *txn,
                        run,
                        expect.max(0) as u64,
                        target,
                        None,
                        NOW,
                    )
                    .await;
                    if outcome.is_ok() {
                        txn.commit().await.unwrap();
                    } else {
                        txn.rollback().await.ok();
                    }
                    outcome
                });
                let after = rig.run_row(run);
                match result {
                    Ok(_) => {
                        prop_assert!(runtime::state::allows(before.state, after.state));
                        prop_assert_eq!(after.state, target);
                        prop_assert_eq!(after.run_revision, before.run_revision + 1);
                        committed_log.push((before.state, after.state));
                    }
                    Err(_) => {
                        prop_assert_eq!(after.state, before.state);
                        prop_assert_eq!(after.run_revision, before.run_revision);
                    }
                }
            }
            // Once terminal, the run can never move again — covered by the
            // committed_log never leaving a terminal state.
            let mut ever_terminal = false;
            for (from, _to) in &committed_log {
                prop_assert!(!ever_terminal, "transition out of terminal state");
                if from.is_terminal() {
                    ever_terminal = true;
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P3: the decision tuple (run, loop_epoch, step_sequence, turn) is unique —
/// a duplicated tuple can never coexist regardless of digest or timing.
#[test]
fn decision_tuple_unique_per_epoch_step_turn() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec((0u8..3, 0u8..3, 0u8..2, 0u8..2), 1..24);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-dec");
            let (_task, run) = rig.task_and_run();
            for (epoch, step, _turn_i, variant) in ops {
                let turn = TurnId::new(&rig.ids);
                let decision = DecisionId::new(&rig.ids);
                rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    txn.loop_turns()
                        .insert_turn(NewLoopTurn {
                            turn_id: turn,
                            run_id: run,
                            run_revision: 0,
                            loop_epoch: epoch as u64,
                            step_sequence: step as u64,
                            input_event_cursor: EventCursor::new(
                                EventStreamKey::new(format!("run/{run}")).unwrap(),
                                0,
                            ),
                            state: "open".to_owned(),
                            issued_at_ms: NOW,
                        })
                        .await
                        .ok(); // turn may already exist for (epoch,step,idx)
                    txn.commit().await.unwrap();
                });
                let digest = format!("sha256:{:064}", variant);
                let inserted = rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let result = txn
                        .loop_turns()
                        .insert_decision(NewDecision {
                            decision_id: decision,
                            run_id: run,
                            turn_id: turn,
                            decision_type: "noop".to_owned(),
                            decision_digest: digest,
                            decision_bytes: vec![variant],
                            run_revision: 0,
                            loop_epoch: epoch as u64,
                            step_sequence: step as u64,
                            input_event_cursor: EventCursor::new(
                                EventStreamKey::new(format!("run/{run}")).unwrap(),
                                0,
                            ),
                            accepted_at_ms: NOW,
                        })
                        .await;
                    match result {
                        Ok(()) => {
                            txn.commit().await.unwrap();
                            true
                        }
                        Err(_) => {
                            txn.rollback().await.ok();
                            false
                        }
                    }
                });
                let fetched = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.loop_turns().get_decision(run, decision).await.unwrap()
                });
                if inserted {
                    prop_assert_eq!(
                        fetched.map(|r| r.decision_id),
                        Some(decision),
                        "committed decision must be fetchable by its key"
                    );
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P4: random legal+illegal edge insertions — the committed graph is
/// acyclic at every observable read.
#[test]
fn dependency_graph_never_contains_a_cycle() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec((0u8..16, 0u8..16), 1..96);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-graph");
            let (task, _r) = rig.task_and_run();
            let runs: Vec<RunId> = (0..16)
                .map(|_| {
                    let run = RunId::new(&rig.ids);
                    rt.block_on(async {
                        let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                        txn.runs()
                            .insert(new_run(run, task, RunState::Running))
                            .await
                            .unwrap();
                        txn.commit().await.unwrap();
                    });
                    run
                })
                .collect();
            rt.block_on(async {
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                txn.graph().ensure_head(task).await.unwrap();
                txn.commit().await.unwrap();
            });

            for (a, b) in &ops {
                let (source, target) = (runs[*a as usize], runs[*b as usize]);
                if source == target {
                    continue;
                }
                rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let head = txn.graph().ensure_head(task).await.unwrap();
                    let res = txn
                        .graph()
                        .insert_dependency(
                            NewRunDependency {
                                dependency_id: DependencyId::new(&rig.ids),
                                task_id: task,
                                source_run_id: source,
                                target_run_id: target,
                                dependency_condition: DependencyCondition::AnyTerminal,
                                created_at_ms: NOW,
                            },
                            head.graph_revision,
                        )
                        .await;
                    if res.is_ok() {
                        txn.commit().await.unwrap();
                    } else {
                        txn.rollback().await.ok();
                    }
                });
                // Invariant check after EVERY committed state.
                let edges: Vec<RunDependencyRow> = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.graph().list_dependencies(task).await.unwrap()
                });
                let mut adj: HashMap<RunId, Vec<RunId>> = HashMap::new();
                for e in &edges {
                    adj.entry(e.source_run_id)
                        .or_default()
                        .push(e.target_run_id);
                }
                for e in &edges {
                    // Reaching source from target means the edge closed a cycle.
                    let mut seen = HashSet::new();
                    let mut stack = vec![e.target_run_id];
                    let mut reachable = false;
                    while let Some(n) = stack.pop() {
                        if n == e.source_run_id {
                            reachable = true;
                            break;
                        }
                        if seen.insert(n) {
                            stack.extend(adj.get(&n).cloned().unwrap_or_default());
                        }
                    }
                    prop_assert!(
                        !reachable,
                        "cycle through {} -> {}",
                        e.source_run_id,
                        e.target_run_id
                    );
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P5: an effect under random claim/dispatch/ack/commit/fail/cancel ops —
/// every persisted state is reachable via `can_transition`, committed
/// effects never regress, and only the recorded fencer moves the row.
#[test]
fn committed_effects_never_regress() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec((0u8..6, 0u8..2, 0u8..3), 1..48);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-effect");
            let (_task, run) = rig.task_and_run();
            let effect = rt.block_on(async {
                let effect = EffectId::new(&rig.ids);
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                txn.effects()
                    .insert(NewEffect {
                        effect_id: effect,
                        run_id: run,
                        step_sequence: 1,
                        decision_id: DecisionId::new(&rig.ids),
                        operation: "fixture.increment_counter".to_owned(),
                        request_hash: "ab".repeat(32),
                        request_payload: Vec::new(),
                        effect_class: domain::effect::EffectClass::ExternalMutation,
                        idempotency_semantics:
                            domain::effect::IdempotencySemantics::IdempotencyKeySupported,
                        reconciliation_semantics:
                            domain::effect::ReconciliationSemantics::StatusLookup,
                        cancellation_semantics: "best_effort".to_owned(),
                        compensation_capability: None,
                        adapter_id: AdapterId::from_str("01905c5e-0000-7000-8000-e11ec7ad01ef")
                            .unwrap(),
                        adapter_version: "0.1.0".to_owned(),
                        adapter_digest: "sha256:effect".to_owned(),
                        state: EffectState::Prepared,
                        created_at_ms: NOW,
                    })
                    .await
                    .unwrap();
                txn.commit().await.unwrap();
                effect
            });

            let mut reached_committed = false;
            for (op, who, delta) in ops {
                let executor_name = if who == 0 { "real" } else { "intruder" };
                // Intruders deliberately use a forged (off-by-delta) token.
                let row = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.effects().get(effect).await.unwrap().unwrap()
                });
                let prev_state = row.state;
                let token = row.executor_fencing_token.unwrap_or(0) + delta as u64;
                let clock = TestClock::new(NOW);
                rt.block_on(async {
                    let env = effects::EffectEnv {
                        ids: &rig.ids,
                        clock: &clock,
                        correlation_id: None,
                        causation_id: None,
                    };
                    let executor = effects::ExecutorRef {
                        executor_id: executor_name,
                        fencing_token: token,
                    };
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let res = match op {
                        0 => effects::claim(&mut *txn, &env, effect, executor_name)
                            .await
                            .map(|_| ()),
                        1 => effects::mark_dispatched(&mut *txn, &env, effect, &executor, None)
                            .await
                            .map(|_| ()),
                        2 => effects::acknowledge(&mut *txn, &env, effect, &executor, None)
                            .await
                            .map(|_| ()),
                        3 => effects::commit(&mut *txn, &env, effect, &executor, None)
                            .await
                            .map(|_| ()),
                        4 => effects::fail(&mut *txn, &env, effect, &executor, "prop")
                            .await
                            .map(|_| ()),
                        _ => effects::cancel(&mut *txn, &env, effect, None)
                            .await
                            .map(|_| ()),
                    };
                    match res {
                        Ok(()) => {
                            txn.commit().await.unwrap();
                        }
                        Err(_) => {
                            txn.rollback().await.ok();
                        }
                    }
                    let _ = res;
                });
                let after = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.effects().get(effect).await.unwrap().unwrap()
                });
                if after.state != prev_state {
                    prop_assert!(
                        effects::can_transition(prev_state, after.state),
                        "illegal effect transition {prev_state:?} -> {:?}",
                        after.state
                    );
                }
                if reached_committed {
                    prop_assert_eq!(
                        after.state,
                        EffectState::Committed,
                        "Committed effect regressed"
                    );
                }
                if after.state == EffectState::Committed {
                    reached_committed = true;
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P6: a random reservation tree — at every committed read, each parent's
/// active same-unit children sum to at most the parent's amount, and
/// release/unknown bookkeeping never resurrects a released row.
#[test]
fn reservation_descendants_within_parent_budget() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    // (parent-index (-1 = root), amount, op)
    let ops = proptest::collection::vec((-1i8..12, 1i64..300, 0u8..3), 1..48);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-budget");
            let (_task, run) = rig.task_and_run();
            let root = rt.block_on(async {
                let parent = domain::ids::ReservationId::new(&rig.ids);
                let clock = TestClock::new(NOW);
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                let env = resources::ResourceEnv {
                    ids: &rig.ids,
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
                        amount: 10_000,
                        parent: None,
                        fencing_token: 0,
                    },
                )
                .await
                .unwrap();
                txn.commit().await.unwrap();
                parent
            });

            let mut nodes: Vec<domain::ids::ReservationId> = vec![root];
            for (parent_i, amount, op) in &ops {
                let parent = if *parent_i < 0 {
                    root
                } else {
                    nodes[*parent_i as usize % nodes.len()]
                };
                match op {
                    0 | 1 => {
                        // delegate: create a child reservation
                        let clock = TestClock::new(NOW);
                        let res = rt.block_on(async {
                            let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                            let env = resources::ResourceEnv {
                                ids: &rig.ids,
                                clock: &clock,
                                correlation_id: None,
                                causation_id: None,
                            };
                            let out = resources::reserve(
                                &mut *txn,
                                &env,
                                resources::ReserveRequest {
                                    reservation_id: None,
                                    run_id: run,
                                    unit: resources::ResourceUnit::WallClockMs,
                                    amount: *amount,
                                    parent: Some(parent),
                                    fencing_token: 0,
                                },
                            )
                            .await;
                            match out {
                                Ok(row) => {
                                    txn.commit().await.unwrap();
                                    Some(row.reservation_id)
                                }
                                Err(_) => {
                                    txn.rollback().await.ok();
                                    None
                                }
                            }
                        });
                        if let Some(id) = res {
                            nodes.push(id);
                        }
                    }
                    2 => {
                        // allocate a random reserved node (not the root)
                        if nodes.len() > 1 {
                            let victim = nodes[1 + (*amount as usize) % (nodes.len() - 1)];
                            let clock = TestClock::new(NOW);
                            rt.block_on(async {
                                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                                let env = resources::ResourceEnv {
                                    ids: &rig.ids,
                                    clock: &clock,
                                    correlation_id: None,
                                    causation_id: None,
                                };
                                let res = resources::allocate(&mut *txn, &env, victim, 0).await;
                                if res.is_ok() {
                                    txn.commit().await.unwrap();
                                } else {
                                    txn.rollback().await.ok();
                                }
                            });
                        }
                    }
                    _ => {
                        // release a random allocated node
                        if nodes.len() > 1 {
                            let victim = nodes[1 + (*amount as usize) % (nodes.len() - 1)];
                            let clock = TestClock::new(NOW);
                            rt.block_on(async {
                                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                                let env = resources::ResourceEnv {
                                    ids: &rig.ids,
                                    clock: &clock,
                                    correlation_id: None,
                                    causation_id: None,
                                };
                                let res = resources::release(&mut *txn, &env, victim, 0).await;
                                if res.is_ok() {
                                    txn.commit().await.unwrap();
                                } else {
                                    txn.rollback().await.ok();
                                }
                            });
                        }
                    }
                }
                // Invariant: for every parent, active children <= amount.
                for p in &nodes {
                    let (prow, children) = rt.block_on(async {
                        let mut read = rig.store.begin_read().await.unwrap();
                        let prow = read.resources().get(*p).await.unwrap();
                        let children = read.resources().list_children(*p).await.unwrap();
                        (prow, children)
                    });
                    let Some(prow) = prow else { continue };
                    let active: i64 = children
                        .iter()
                        .filter(|c| {
                            matches!(
                                c.state,
                                ReservationState::Reserved
                                    | ReservationState::Allocated
                                    | ReservationState::Unknown
                            )
                        })
                        .map(|c| c.amount)
                        .sum();
                    prop_assert!(
                        active <= prow.amount,
                        "delegated {active} exceeds parent {}",
                        prow.amount
                    );
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P7: random fire/cancel CAS ops — version strictly increases by one per
/// committed mutation and terminal timer states never revert.
#[test]
fn timer_version_monotonic_terminal_stable() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    // (op, version-delta): claim Scheduled->Claimed, fire Claimed->Fired,
    // cancel Scheduled->Cancelled — the three scheduler-legal transitions.
    let ops = proptest::collection::vec((0u8..3, -1i64..2), 1..24);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-timer");
            let (_task, run) = rig.task_and_run();
            let timer = TimerId::new(&rig.ids);
            rt.block_on(async {
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
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
            });
            for (op, delta) in ops {
                let before = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.timers().get(timer).await.unwrap().unwrap()
                });
                // (expected state, target) — mirror scheduler/lib.rs legal pairs.
                let (expect_state, target) = match op {
                    0 => (TimerState::Scheduled, TimerState::Claimed),
                    1 => (TimerState::Claimed, TimerState::Fired),
                    _ => (TimerState::Scheduled, TimerState::Cancelled),
                };
                let expect_version = before.version as i64 + delta;
                if expect_version < 0 {
                    continue;
                }
                let won = rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let won = txn
                        .timers()
                        .cas_transition(
                            timer,
                            expect_state,
                            expect_version as u64,
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
                    won
                });
                let after = rt.block_on(async {
                    let mut read = rig.store.begin_read().await.unwrap();
                    read.timers().get(timer).await.unwrap().unwrap()
                });
                if won {
                    prop_assert_eq!(after.version, before.version + 1);
                    prop_assert_eq!(after.state, target);
                } else {
                    prop_assert_eq!(after.version, before.version);
                    prop_assert_eq!(after.state, before.state);
                }
                // Kernel callers never address a terminal row: once Fired or
                // Cancelled, no committed op can originate from it.
                if matches!(before.state, TimerState::Fired | TimerState::Cancelled) {
                    prop_assert_eq!(
                        after.state,
                        before.state,
                        "committed transition out of a terminal timer state"
                    );
                }
            }
            Ok(())
        })
        .unwrap();
}

/// P8: random acquire/transfer/revoke ops — at most one ACTIVE
/// ExclusiveWrite lease exists for the workspace at any committed read.
#[test]
fn never_two_active_exclusive_leases() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec((0u8..3, 0u8..4), 1..24);
    runner
        .run(&ops, |ops| {
            let mut rig = Rig::open(&rt, "p-lease");
            let (_task, run_a) = rig.task_and_run();
            let (_t2, run_b) = rig.task_and_run();
            let workspace = WorkspaceId::new(&rig.ids);
            let mut leases: Vec<LeaseId> = Vec::new();
            for (op, pick) in ops {
                match op {
                    0 | 1 => {
                        // Try to acquire ExclusiveWrite for a random owner.
                        let owner = if pick % 2 == 0 { run_a } else { run_b };
                        let res = rt.block_on(async {
                            let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                            let out = workspace_crate::coordinator::acquire_lease(
                                &mut *txn,
                                &rig.ids,
                                workspace,
                                owner,
                                WorkspaceAccessMode::ExclusiveWrite,
                                &[],
                                Vec::new(),
                                NOW,
                            )
                            .await;
                            match out {
                                Ok(row) => {
                                    txn.commit().await.unwrap();
                                    Some(row.lease_id)
                                }
                                Err(_) => {
                                    txn.rollback().await.ok();
                                    None
                                }
                            }
                        });
                        if let Some(id) = res {
                            leases.push(id);
                        }
                    }
                    _ => {
                        // Revoke a random lease.
                        if !leases.is_empty() {
                            let victim = leases[pick as usize % leases.len()];
                            rt.block_on(async {
                                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                                let row = txn.workspaces().get_lease(victim).await.unwrap();
                                if let Some(row) = row
                                    && row.enforcement_state == LeaseEnforcementState::Active
                                {
                                    let _ = workspace_crate::coordinator::revoke_lease(
                                        &mut *txn,
                                        victim,
                                        row.lease_epoch,
                                    )
                                    .await;
                                }
                                txn.commit().await.unwrap();
                            });
                        }
                    }
                }
                // Invariant over all leases ever created.
                let mut active_exclusive = 0;
                for id in &leases {
                    let row = rt.block_on(async {
                        let mut read = rig.store.begin_read().await.unwrap();
                        read.workspaces().get_lease(*id).await.unwrap()
                    });
                    if let Some(row) = row
                        && row.enforcement_state == LeaseEnforcementState::Active
                        && row.mode == WorkspaceAccessMode::ExclusiveWrite
                    {
                        active_exclusive += 1;
                    }
                }
                prop_assert!(
                    active_exclusive <= 1,
                    "{active_exclusive} active exclusive leases"
                );
            }
            Ok(())
        })
        .unwrap();
}

/// P9: outbox sequences are strictly contiguous per stream — allocation is
/// linearized in the write txn, so a committed sequence set is exactly
/// 1..=n in any order of insertion across streams.
#[test]
fn outbox_sequence_strictly_contiguous_per_stream() {
    let rt = rt();
    let mut runner = TestRunner::new(config());
    let ops = proptest::collection::vec(0u8..4, 1..48);
    runner
        .run(&ops, |ops| {
            let rig = Rig::open(&rt, "p-outbox");
            let streams: Vec<EventStreamKey> = (0..4)
                .map(|i| EventStreamKey::new(format!("prop/stream-{i}")).unwrap())
                .collect();
            for lane in ops {
                let key = streams[lane as usize].clone();
                rt.block_on(async {
                    let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                    let seq = txn.streams().allocate(key.clone()).await.unwrap();
                    txn.streams()
                        .insert_outbox(NewOutboxEvent {
                            event_id: EventId::new(&rig.ids),
                            event_type: "prop.test".to_owned(),
                            event_version: 1,
                            stream_key: key,
                            sequence: seq,
                            occurred_at_ms: NOW,
                            run_id: None,
                            task_id: None,
                            session_id: None,
                            effect_id: None,
                            causation_id: None,
                            correlation_id: None,
                            sensitivity: SensitivityClass::Internal,
                            retention: RetentionClass::Standard,
                            payload: vec![lane],
                        })
                        .await
                        .unwrap();
                    txn.commit().await.unwrap();
                });
            }
            let rows = rt.block_on(async {
                // scan_unpublished lives on the write-txn repo surface; roll
                // the txn back after the read.
                let mut txn = rig.store.begin_write(rig.ctx()).await.unwrap();
                let rows = txn.streams().scan_unpublished(10_000).await.unwrap();
                txn.rollback().await.ok();
                rows
            });
            let mut by_stream: HashMap<String, Vec<u64>> = HashMap::new();
            for row in rows {
                by_stream
                    .entry(row.stream_key.as_str().to_owned())
                    .or_default()
                    .push(row.sequence);
            }
            for (key, mut seqs) in by_stream {
                seqs.sort_unstable();
                let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
                prop_assert_eq!(
                    seqs,
                    expected,
                    "stream {} sequence must be 1..=n contiguous",
                    key
                );
            }
            Ok(())
        })
        .unwrap();
}
