//! RES-001: durable reservations, delegation budgets, fencing.

use std::sync::Arc;

use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventCursor, EventStreamKey, PrincipalId, ReservationId,
    RunId, TaskId,
};
use domain::resource::ReservationState;
use domain::run::{RecoveryDisposition, RunState};
use errors::codes::ErrorCode;
use kernel_store::models::{NewRun, NewTask, ReservationRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use resources::{ReserveRequest, ResourceEnv, ResourceUnit};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: DeterministicIds,
    clock: TestClock,
    principal: PrincipalId,
    actor: ActorId,
    epoch: u64,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp root");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join("kernel.db"),
                pool_max_connections: 8,
                busy_timeout_ms: 30_000,
            })
            .await
            .expect("store opens"),
        );
        let ids = DeterministicIds::new(SEED_MS);
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(&ids))
            .await
            .expect("daemon fence acquired");
        let principal = PrincipalId::new(&ids);
        let actor = ActorId::new(&ids);
        Self {
            _dir: dir,
            store,
            ids,
            clock: TestClock::new(SEED_MS),
            principal,
            actor,
            epoch: fence.epoch.0,
        }
    }

    async fn write(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(TxContext {
                daemon_epoch: self.epoch,
                principal_id: self.principal,
                command_id: CommandId::new(&self.ids),
                correlation_id: None,
            })
            .await
            .expect("write txn")
    }

    fn env(&self) -> ResourceEnv<'_> {
        ResourceEnv {
            ids: &self.ids,
            clock: &self.clock,
            correlation_id: None,
            causation_id: None,
        }
    }

    async fn seed_run(&self) -> RunId {
        let task_id = TaskId::new(&self.ids);
        let run_id = RunId::new(&self.ids);
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("canonical stream key"),
            0,
        );
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: self.actor,
                task_kind: "resources".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id,
                session_id: None,
                parent_run_id: None,
                state: RunState::Running,
                recovery: RecoveryDisposition::Normal,
                loop_epoch: 0,
                step_sequence: 0,
                input_event_cursor: cursor,
                cancellation_epoch: 0,
                resolved_environment_id: None,
                agent_spec_id: None,
                agent_spec_version: None,
                agent_spec_digest: None,
                requested_profile: String::new(),
                workspace_uri: None,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed run");
        txn.commit().await.expect("commit");
        run_id
    }

    async fn reserve(
        &self,
        run_id: RunId,
        unit: ResourceUnit,
        amount: i64,
        parent: Option<ReservationId>,
        fencing_token: u64,
    ) -> errors::Result<ReservationRow> {
        let mut txn = self.write().await;
        let row = resources::reserve(
            txn.as_mut(),
            &self.env(),
            ReserveRequest {
                reservation_id: None,
                run_id,
                unit,
                amount,
                parent,
                fencing_token,
            },
        )
        .await?;
        txn.commit().await.expect("commit");
        Ok(row)
    }

    async fn state(&self, id: ReservationId) -> ReservationState {
        let mut txn = self.write().await;
        let state = txn
            .resources()
            .get(id)
            .await
            .expect("read")
            .expect("reservation")
            .state;
        txn.rollback().await.expect("rollback");
        state
    }
}

#[tokio::test]
async fn over_delegation_is_rejected() {
    let h = Harness::new().await;
    let run = h.seed_run().await;
    let parent = h
        .reserve(run, ResourceUnit::SandboxSlots, 4, None, 0)
        .await
        .expect("parent");
    h.reserve(
        run,
        ResourceUnit::SandboxSlots,
        3,
        Some(parent.reservation_id),
        0,
    )
    .await
    .expect("first child fits");
    let err = h
        .reserve(
            run,
            ResourceUnit::SandboxSlots,
            2,
            Some(parent.reservation_id),
            0,
        )
        .await
        .expect_err("3+2 exceeds 4");
    assert_eq!(err.code(), ErrorCode::ResourceExhausted);
    // Boundary: exactly the remaining budget still fits.
    h.reserve(
        run,
        ResourceUnit::SandboxSlots,
        1,
        Some(parent.reservation_id),
        0,
    )
    .await
    .expect("remaining 1 fits");
}

#[tokio::test]
async fn released_budget_can_be_re_reserved() {
    let h = Harness::new().await;
    let run = h.seed_run().await;
    let parent = h
        .reserve(run, ResourceUnit::ModelTokens, 2, None, 0)
        .await
        .expect("parent");
    let child = h
        .reserve(
            run,
            ResourceUnit::ModelTokens,
            2,
            Some(parent.reservation_id),
            0,
        )
        .await
        .expect("child fills the budget");
    // Allocate then release frees the delegated amount.
    let mut txn = h.write().await;
    resources::allocate(txn.as_mut(), &h.env(), child.reservation_id, 0)
        .await
        .expect("allocate");
    resources::release(txn.as_mut(), &h.env(), child.reservation_id, 0)
        .await
        .expect("release");
    txn.commit().await.expect("commit");
    assert_eq!(
        h.state(child.reservation_id).await,
        ReservationState::Released
    );
    // A fresh child may re-reserve the full budget.
    h.reserve(
        run,
        ResourceUnit::ModelTokens,
        2,
        Some(parent.reservation_id),
        0,
    )
    .await
    .expect("released budget re-reservable");
}

#[tokio::test]
async fn concurrent_children_cannot_exceed_parent() {
    let h = Arc::new(Harness::new().await);
    let run = h.seed_run().await;
    let parent = h
        .reserve(run, ResourceUnit::ChildRunSlots, 2, None, 0)
        .await
        .expect("parent");
    // Ten racing children for a budget of 2: at most 2 may succeed.
    let mut joins = Vec::new();
    for _ in 0..10 {
        let h = Arc::clone(&h);
        joins.push(tokio::spawn(async move {
            h.reserve(
                run,
                ResourceUnit::ChildRunSlots,
                1,
                Some(parent.reservation_id),
                0,
            )
            .await
        }));
    }
    let mut won = 0;
    let mut lost = 0;
    for join in joins {
        match join
            .await
            .expect("task")
            .map_err(|e: errors::KernelError| e.code())
        {
            Ok(_) => won += 1,
            Err(ErrorCode::ResourceExhausted | ErrorCode::Conflict | ErrorCode::Unavailable) => {
                lost += 1
            }
            Err(code) => panic!("unexpected error {code:?}"),
        }
    }
    assert_eq!(won, 2, "racing children may not exceed the budget");
    assert_eq!(lost, 8);
}

#[tokio::test]
async fn unknown_external_allocation_recovers_by_refencing() {
    let h = Harness::new().await;
    let run = h.seed_run().await;
    let parent = h
        .reserve(run, ResourceUnit::SandboxSlots, 1, None, 0)
        .await
        .expect("parent");
    let child = h
        .reserve(
            run,
            ResourceUnit::SandboxSlots,
            1,
            Some(parent.reservation_id),
            7,
        )
        .await
        .expect("external reservation");
    let mut txn = h.write().await;
    resources::allocate(txn.as_mut(), &h.env(), child.reservation_id, 7)
        .await
        .expect("allocated by token owner");
    // Owner crashes mid-allocation: the outcome is uncertain.
    resources::mark_unknown(txn.as_mut(), &h.env(), child.reservation_id)
        .await
        .expect("unknown");
    txn.commit().await.expect("commit");
    // The uncertain child still counts against the budget — nothing frees it.
    let err = h
        .reserve(
            run,
            ResourceUnit::SandboxSlots,
            1,
            Some(parent.reservation_id),
            0,
        )
        .await
        .expect_err("unknown allocations still hold budget");
    assert_eq!(err.code(), ErrorCode::ResourceExhausted);
    // Recovery re-fences to the new owner; the stale token loses authority.
    let mut txn = h.write().await;
    resources::recover_unknown(txn.as_mut(), &h.env(), child.reservation_id, 9)
        .await
        .expect("recover");
    txn.commit().await.expect("commit");
    let mut txn = h.write().await;
    let err = resources::release(txn.as_mut(), &h.env(), child.reservation_id, 7)
        .await
        .expect_err("stale fencing token cannot release");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    resources::release(txn.as_mut(), &h.env(), child.reservation_id, 9)
        .await
        .expect("new owner releases");
    txn.commit().await.expect("commit");
    assert_eq!(
        h.state(child.reservation_id).await,
        ReservationState::Released
    );
}

#[tokio::test]
async fn mismatched_unit_or_dead_parent_is_rejected() {
    let h = Harness::new().await;
    let run = h.seed_run().await;
    let parent = h
        .reserve(run, ResourceUnit::DiskBytes, 10, None, 0)
        .await
        .expect("parent");
    let err = h
        .reserve(
            run,
            ResourceUnit::WallClockMs,
            1,
            Some(parent.reservation_id),
            0,
        )
        .await
        .expect_err("cross-unit delegation");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    // A terminal parent cannot delegate.
    let mut txn = h.write().await;
    resources::allocate(txn.as_mut(), &h.env(), parent.reservation_id, 0)
        .await
        .expect("allocate");
    resources::release(txn.as_mut(), &h.env(), parent.reservation_id, 0)
        .await
        .expect("release");
    txn.commit().await.expect("commit");
    let err = h
        .reserve(
            run,
            ResourceUnit::DiskBytes,
            1,
            Some(parent.reservation_id),
            0,
        )
        .await
        .expect_err("released parent cannot delegate");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn replay_with_different_fencing_token_conflicts() {
    let h = Harness::new().await;
    let run = h.seed_run().await;
    let reservation_id = ReservationId::new(&h.ids);
    let request = |token: u64| ReserveRequest {
        reservation_id: Some(reservation_id),
        run_id: run,
        unit: ResourceUnit::SandboxSlots,
        amount: 1,
        parent: None,
        fencing_token: token,
    };

    let mut txn = h.write().await;
    let stored = resources::reserve(txn.as_mut(), &h.env(), request(7))
        .await
        .expect("first reserve");
    txn.commit().await.expect("commit");
    assert_eq!(stored.fencing_token, 7);

    // Identical replay returns the stored row.
    let mut txn = h.write().await;
    let replayed = resources::reserve(txn.as_mut(), &h.env(), request(7))
        .await
        .expect("identical replay");
    txn.commit().await.expect("commit");
    assert_eq!(replayed.reservation_id, reservation_id);

    // A replay claiming a different external owner must not pass as
    // identical — the persisted row still belongs to token 7.
    let mut txn = h.write().await;
    let err = resources::reserve(txn.as_mut(), &h.env(), request(9))
        .await
        .expect_err("divergent token conflicts");
    txn.rollback().await.expect("rollback");
    assert_eq!(err.code(), ErrorCode::Conflict);
}
