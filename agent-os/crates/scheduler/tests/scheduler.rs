//! SCH-001: durable timer semantics over the real sqlite store.

use std::sync::Arc;

use domain::ids::{CommandId, DaemonInstanceId, PrincipalId, TimerId};
use domain::resource::TimerState;
use domain::time::Clock;
use errors::codes::ErrorCode;
use kernel_store::models::TimerRow;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use scheduler::{ScheduleRequest, SchedulerEnv};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    ids: DeterministicIds,
    clock: TestClock,
    principal: PrincipalId,
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
        Self {
            _dir: dir,
            store,
            ids,
            clock: TestClock::new(SEED_MS),
            principal,
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

    fn env(&self) -> SchedulerEnv<'_> {
        SchedulerEnv {
            ids: &self.ids,
            clock: &self.clock,
            correlation_id: None,
            causation_id: None,
        }
    }

    async fn timer_state(&self, timer_id: TimerId) -> TimerState {
        let mut txn = self.write().await;
        let state = txn
            .timers()
            .get(timer_id)
            .await
            .expect("read")
            .expect("timer")
            .state;
        txn.rollback().await.expect("rollback");
        state
    }
}

async fn schedule_timer(h: &Harness, due_at_ms: i64) -> TimerRow {
    let mut txn = h.write().await;
    let row = scheduler::schedule(
        txn.as_mut(),
        &h.env(),
        ScheduleRequest {
            timer_id: None,
            run_id: None,
            timer_kind: "test.fire".to_owned(),
            due_at_ms,
            payload: b"cmd".to_vec(),
        },
    )
    .await
    .expect("schedule");
    txn.commit().await.expect("commit");
    row
}

#[tokio::test]
async fn schedule_is_retry_safe_and_conflicts_on_divergence() {
    let h = Harness::new().await;
    let row = schedule_timer(&h, SEED_MS + 1_000).await;
    assert_eq!(row.state, TimerState::Scheduled);
    let mut txn = h.write().await;
    // Identical replay returns the stored row.
    let again = scheduler::schedule(
        txn.as_mut(),
        &h.env(),
        ScheduleRequest {
            timer_id: Some(row.timer_id),
            run_id: None,
            timer_kind: "test.fire".to_owned(),
            due_at_ms: SEED_MS + 1_000,
            payload: b"cmd".to_vec(),
        },
    )
    .await
    .expect("identical replay is idempotent");
    assert_eq!(again.version, row.version);
    // Divergent reuse conflicts.
    let err = scheduler::schedule(
        txn.as_mut(),
        &h.env(),
        ScheduleRequest {
            timer_id: Some(row.timer_id),
            run_id: None,
            timer_kind: "test.fire".to_owned(),
            due_at_ms: SEED_MS + 2_000,
            payload: b"cmd".to_vec(),
        },
    )
    .await
    .expect_err("different due time for the same id conflicts");
    assert_eq!(err.code(), ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn claim_and_cancel_race_has_exactly_one_winner() {
    let h = Harness::new().await;
    let row = schedule_timer(&h, SEED_MS - 1).await;
    let claimed = {
        let mut txn = h.write().await;
        let claimed = scheduler::claim(txn.as_mut(), &h.env(), row.timer_id, "w1", h.epoch)
            .await
            .expect("claim");
        txn.commit().await.expect("commit");
        claimed
    };
    let cancel = {
        let mut txn = h.write().await;
        let outcome = scheduler::cancel(txn.as_mut(), &h.env(), row.timer_id, row.version).await;
        txn.rollback().await.expect("rollback");
        outcome
    };
    assert!(claimed.is_some(), "first claim wins");
    assert_eq!(
        cancel.expect_err("claimed timer cannot cancel").code(),
        ErrorCode::FailedPrecondition
    );
    assert_eq!(h.timer_state(row.timer_id).await, TimerState::Claimed);
}

#[tokio::test]
async fn cancel_wins_when_claim_loses_the_race() {
    let h = Harness::new().await;
    let row = schedule_timer(&h, SEED_MS - 1).await;
    let mut txn = h.write().await;
    scheduler::cancel(txn.as_mut(), &h.env(), row.timer_id, row.version)
        .await
        .expect("cancel scheduled");
    txn.commit().await.expect("commit");
    let mut txn = h.write().await;
    let claim = scheduler::claim(txn.as_mut(), &h.env(), row.timer_id, "w1", h.epoch)
        .await
        .expect("claim");
    txn.rollback().await.expect("rollback");
    assert!(claim.is_none());
    assert_eq!(h.timer_state(row.timer_id).await, TimerState::Cancelled);
    // Repeated cancel returns the stored outcome rather than failing.
    let mut txn = h.write().await;
    let replay = scheduler::cancel(txn.as_mut(), &h.env(), row.timer_id, row.version)
        .await
        .expect("repeat cancel replays");
    assert_eq!(replay.state, TimerState::Cancelled);
    txn.rollback().await.expect("rollback");
    // A stale expected version on a Scheduled timer also conflicts.
    let fresh = schedule_timer(&h, SEED_MS - 1).await;
    let mut txn = h.write().await;
    let err = scheduler::cancel(txn.as_mut(), &h.env(), fresh.timer_id, fresh.version + 9)
        .await
        .expect_err("version mismatch fails");
    assert_eq!(err.code(), ErrorCode::Conflict);
    txn.rollback().await.expect("rollback");
}

#[tokio::test]
async fn double_fire_is_prevented() {
    let h = Harness::new().await;
    let row = schedule_timer(&h, SEED_MS - 1).await;
    let claimed = {
        let mut txn = h.write().await;
        let claimed = scheduler::claim(txn.as_mut(), &h.env(), row.timer_id, "w1", h.epoch)
            .await
            .expect("claim")
            .expect("claimed");
        txn.commit().await.expect("commit");
        claimed
    };
    let fired = {
        let mut txn = h.write().await;
        let won = scheduler::mark_fired(txn.as_mut(), &h.env(), &claimed)
            .await
            .expect("fire");
        txn.commit().await.expect("commit");
        won
    };
    assert!(fired);
    let again = {
        let mut txn = h.write().await;
        let won = scheduler::mark_fired(txn.as_mut(), &h.env(), &claimed)
            .await
            .expect("refire");
        txn.rollback().await.expect("rollback");
        won
    };
    assert!(!again, "stale claim CAS loses the second fire");
    assert_eq!(h.timer_state(row.timer_id).await, TimerState::Fired);
}

#[tokio::test]
async fn restart_after_claim_recovers_safely() {
    let h = Harness::new().await;
    let row = schedule_timer(&h, SEED_MS - 1).await;
    // Epoch 1 claims, then "crashes" (its claim stays durable).
    {
        let mut txn = h.write().await;
        let claimed =
            scheduler::claim_next_due(txn.as_mut(), &h.env(), h.clock.now_unix_ms(), "w1", 1)
                .await
                .expect("claim");
        txn.commit().await.expect("commit");
        assert!(claimed.is_some());
    }
    assert_eq!(h.timer_state(row.timer_id).await, TimerState::Claimed);
    // Epoch 2 scans: the stale claim is re-fenced and dispatchable again.
    let mut txn = h.write().await;
    let reclaimed =
        scheduler::claim_next_due(txn.as_mut(), &h.env(), h.clock.now_unix_ms(), "w2", 2)
            .await
            .expect("reclaim")
            .expect("stale claim is recoverable");
    assert_eq!(reclaimed.claim_daemon_epoch, Some(2));
    assert_eq!(reclaimed.claim_owner.as_deref(), Some("w2"));
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn not_due_timer_is_not_claimable() {
    let h = Harness::new().await;
    let _row = schedule_timer(&h, SEED_MS + 60_000).await;
    let mut txn = h.write().await;
    let claimed = scheduler::claim_next_due(txn.as_mut(), &h.env(), h.clock.now_unix_ms(), "w", 1)
        .await
        .expect("scan");
    txn.rollback().await.expect("rollback");
    assert!(claimed.is_none());
}
