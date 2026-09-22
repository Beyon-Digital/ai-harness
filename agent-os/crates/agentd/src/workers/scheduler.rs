//! Scheduler worker.
//!
//! One iteration claims the next dispatchable timer (winning `Scheduled ->
//! Claimed`, re-owning its own pending claim, or re-fencing a claim left by
//! a dead daemon epoch), submits the row's `timer_kind`/`payload` as a
//! normal internal kernel command through the coordinator, then commits
//! `Fired` if the claim is still current. The command's idempotency key is
//! `timer.fire.<timer_id>`, so a crash between dispatch and commit replays
//! to the stored outcome — the timer's effect happens at most once while
//! the row is only ever fired once.
//!
//! The composition root wires this worker in a later task, so its items are
//! not yet reachable from `main`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use std::str::FromStr;

use command_coordinator::CommandCoordinator;
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use domain::ids::{ActorId, CommandId, IdempotencyKey, PrincipalId};
use domain::provider::IdProvider;
use domain::time::Clock;
use kernel_store::KernelStore;
use kernel_store::TxContext;
use kernel_store::models::TimerRow;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval};

use crate::workers::outbox::EpochSource;

/// Poll interval bound for due-timer scans.
const BATCH_LIMIT_PER_TICK: usize = 64;

/// Shared dependencies of the scheduler worker.
pub struct SchedulerWorkerDeps {
    /// Durable store.
    pub store: Arc<dyn KernelStore>,
    /// Envelope dispatch path for fired commands.
    pub coordinator: Arc<CommandCoordinator>,
    /// Daemon fencing epoch the worker claims under.
    pub epoch: Arc<SchedulerEpochSource>,
    /// Id source for internal commands.
    pub ids: Arc<dyn IdProvider>,
    /// Wall clock for due checks.
    pub clock: Arc<dyn Clock>,
    /// Internal principal for worker commands.
    pub principal: PrincipalId,
    /// Internal actor for worker commands.
    pub actor: ActorId,
}

/// Supplies the daemon fencing epoch the worker claims under.
pub type SchedulerEpochSource = dyn EpochSource;

/// Interval loop that claims due timers, submits their kernel commands, and
/// commits `Fired`.
#[allow(dead_code)]
pub struct SchedulerWorker {
    store: Arc<dyn KernelStore>,
    coordinator: Arc<CommandCoordinator>,
    epoch: Arc<SchedulerEpochSource>,
    poll: Duration,
    ids: Arc<dyn IdProvider>,
    clock: Arc<dyn Clock>,
    principal: PrincipalId,
    actor: ActorId,
    /// Per-timer retry state: `timer_id -> (attempts, next_attempt_unix_ms)`.
    /// A permanently failing claim is deferred with backoff so it cannot
    /// starve every later due timer by staying first-selectable.
    backoff: HashMap<domain::ids::TimerId, (u32, i64)>,
}

impl SchedulerWorker {
    /// Creates a worker claiming timers as `owner` under `epoch`.
    #[allow(dead_code)]
    pub fn new(deps: SchedulerWorkerDeps, poll: Duration) -> Self {
        Self {
            store: deps.store,
            coordinator: deps.coordinator,
            epoch: deps.epoch,
            poll,
            ids: deps.ids,
            clock: deps.clock,
            principal: deps.principal,
            actor: deps.actor,
            backoff: HashMap::new(),
        }
    }

    /// Dispatches timers until `shutdown` requests a stop.
    #[allow(dead_code)]
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        let mut ticker = interval(self.poll);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                    continue;
                }
            }
            // A claim failure mid-tick just retries on the next tick; a timer
            // is never lost because its row is durable.
            let _ = self.dispatch_tick().await;
        }
    }

    /// Claims and dispatches up to `BATCH_LIMIT_PER_TICK` due timers. A
    /// timer whose fire fails is deferred with exponential backoff and
    /// excluded for the rest of the tick, so one bad row cannot stall the
    /// queue by staying the earliest selectable claim.
    async fn dispatch_tick(&mut self) -> errors::Result<()> {
        let owner = format!("scheduler:{}", self.actor);
        let now = self.clock.now_unix_ms();
        let mut exclude: HashSet<domain::ids::TimerId> = self
            .backoff
            .iter()
            .filter(|(_, (_, next))| *next > now)
            .map(|(id, _)| *id)
            .collect();
        for _ in 0..BATCH_LIMIT_PER_TICK {
            let Some(claimed) = self.claim_next(&owner, &exclude).await? else {
                return Ok(());
            };
            if let Err(_error) = self.fire(claimed.clone(), &owner).await {
                let attempts = self
                    .backoff
                    .get(&claimed.timer_id)
                    .map(|(n, _)| *n)
                    .unwrap_or(0)
                    .saturating_add(1);
                // 2s, 4s, 8s, ... capped at 60s — a deterministic retry
                // schedule so restart behavior stays explainable.
                let shift = attempts.saturating_sub(1).min(5);
                let delay_ms = 2_000i64.saturating_mul(1i64 << shift).min(60_000);
                self.backoff
                    .insert(claimed.timer_id, (attempts, now + delay_ms));
                exclude.insert(claimed.timer_id);
                continue;
            }
            self.backoff.remove(&claimed.timer_id);
        }
        Ok(())
    }

    /// Claims the next dispatchable timer inside one transaction.
    async fn claim_next(
        &self,
        owner: &str,
        exclude: &HashSet<domain::ids::TimerId>,
    ) -> errors::Result<Option<TimerRow>> {
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch: self.epoch.epoch(),
                principal_id: self.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await?;
        let env = scheduler::SchedulerEnv {
            ids: self.ids.as_ref(),
            clock: self.clock.as_ref(),
            correlation_id: None,
            causation_id: None,
        };
        let claimed = scheduler::claim_next_due(
            txn.as_mut(),
            &env,
            self.clock.now_unix_ms(),
            owner,
            self.epoch.epoch(),
            exclude,
        )
        .await?;
        txn.commit().await?;
        Ok(claimed)
    }

    /// Submits the timer's kernel command and commits `Fired` if the claim
    /// is still current.
    async fn fire(&self, claimed: TimerRow, _owner: &str) -> errors::Result<()> {
        let digest_hex: String = Sha256::digest(&claimed.payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let envelope = CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(scheduler::fire_idempotency_key(claimed.timer_id))
                .expect("derived idempotency key is well formed"),
            principal_id: self.principal,
            actor_id: self.actor,
            device_id: None,
            delegation_chain_id: None,
            request_digest: RequestDigest::from_str(&digest_hex)
                .expect("sha256 hex is a valid request digest"),
            correlation_id: Some(format!("timer/{}", claimed.timer_id)),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: claimed.timer_kind.clone(),
            payload: claimed.payload.clone(),
        };
        self.coordinator.execute(envelope).await?;
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch: self.epoch.epoch(),
                principal_id: self.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await?;
        let env = scheduler::SchedulerEnv {
            ids: self.ids.as_ref(),
            clock: self.clock.as_ref(),
            correlation_id: None,
            causation_id: None,
        };
        scheduler::mark_fired(txn.as_mut(), &env, &claimed).await?;
        txn.commit().await?;
        Ok(())
    }
}
