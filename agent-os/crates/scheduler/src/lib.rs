//! Durable one-shot timers: schedule, claim, fire, and cancel.
//!
//! A timer is `Scheduled -> Claimed -> Fired` (or `Scheduled -> Cancelled`).
//! Claim and cancel both CAS from `Scheduled` at an expected version, so a
//! race has exactly one winner (scheduler.md). A fired timer emits no run
//! mutation itself: the owning worker submits the row's `timer_kind` +
//! `payload` as a normal kernel command, then commits `Fired` if the claim
//! is still current — with a deterministic idempotency key so a crash
//! between dispatch and commit replays to the same stored outcome.
#![forbid(unsafe_code)]

mod stage;

use domain::generated::contract;
use domain::ids::{EventId, RunId, TimerId};
use domain::provider::IdProvider;
use domain::resource::TimerState;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::{NewTimer, TimerPatch, TimerRow};
use prost::Message;
use stage::stage_timer_event;

/// Fully-qualified command type of `ScheduleTimer`.
pub const CMD_SCHEDULE_TIMER: &str = "agentos.spec.v1.ScheduleTimer";
/// Fully-qualified command type of `CancelTimer`.
pub const CMD_CANCEL_TIMER: &str = "agentos.spec.v1.CancelTimer";

/// Deterministic idempotency key for the kernel command a fired timer
/// submits; a restarted worker redispatching the same fire dedupes at the
/// coordinator.
pub fn fire_idempotency_key(timer_id: TimerId) -> String {
    format!("timer.fire.{timer_id}")
}

/// Clock/id environment shared by every timer mutation.
pub struct SchedulerEnv<'a> {
    /// Identifier source for timer rows and outbox events.
    pub ids: &'a dyn IdProvider,
    /// Clock for `created_at_ms`/`updated_at_ms`.
    pub clock: &'a dyn Clock,
    /// Correlation id propagated onto staged events.
    pub correlation_id: Option<String>,
    /// Causing event, when the mutation itself was event-driven.
    pub causation_id: Option<EventId>,
}

/// Fields of a `ScheduleTimer` command after envelope decoding.
pub struct ScheduleRequest {
    /// Caller-supplied id; minted when absent.
    pub timer_id: Option<TimerId>,
    /// Run the timer belongs to (stream target); `None` for daemon timers.
    pub run_id: Option<RunId>,
    /// Command type the fire submits.
    pub timer_kind: String,
    /// Unix ms at which the timer becomes due.
    pub due_at_ms: i64,
    /// Opaque command payload carried to the fired command.
    pub payload: Vec<u8>,
}

/// Schedules a timer inside the caller's transaction.
///
/// Retry-safe on a caller-supplied `timer_id`: an identical replay returns
/// the stored row; the same id with different fields is `Conflict`.
pub async fn schedule(
    txn: &mut dyn KernelTxn,
    env: &SchedulerEnv<'_>,
    request: ScheduleRequest,
) -> errors::Result<TimerRow> {
    let timer_id = request.timer_id.unwrap_or_else(|| TimerId::new(env.ids));
    if let Some(existing) = txn.timers().get(timer_id).await? {
        let identical = existing.run_id == request.run_id
            && existing.timer_kind == request.timer_kind
            && existing.due_at_ms == request.due_at_ms
            && existing.payload == request.payload;
        if identical {
            return Ok(existing);
        }
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "timer_id already scheduled with different fields",
        ));
    }
    let now_ms = env.clock.now_unix_ms();
    txn.timers()
        .insert(NewTimer {
            timer_id,
            run_id: request.run_id,
            timer_kind: request.timer_kind.clone(),
            payload: request.payload,
            due_at_ms: request.due_at_ms,
            state: TimerState::Scheduled,
            version: 0,
            created_at_ms: now_ms,
        })
        .await?;
    let row = txn
        .timers()
        .get(timer_id)
        .await?
        .expect("timer visible in its own transaction");
    stage_timer_event(
        txn,
        EventId::new(env.ids),
        "TimerScheduled",
        timer_stream(request.run_id),
        timer_payload(&row),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(row)
}

/// Cancels a `Scheduled` timer via `(state, version)` CAS.
///
/// Exactly one of claim and cancel wins. A repeated cancel of an already
/// `Cancelled` timer returns the stored outcome; a version mismatch or a
/// lost race is `Conflict`, and a timer in a terminal-but-uncancelled state
/// is `FailedPrecondition`.
pub async fn cancel(
    txn: &mut dyn KernelTxn,
    env: &SchedulerEnv<'_>,
    timer_id: TimerId,
    expected_version: u64,
) -> errors::Result<TimerRow> {
    let row = txn
        .timers()
        .get(timer_id)
        .await?
        .ok_or_else(|| missing(timer_id))?;
    if row.state == TimerState::Cancelled {
        return Ok(row);
    }
    if row.state != TimerState::Scheduled {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("timer in state {} cannot be cancelled", row.state.as_str()),
        ));
    }
    let won = txn
        .timers()
        .cas_transition(
            timer_id,
            TimerState::Scheduled,
            expected_version,
            TimerPatch {
                state: Some(TimerState::Cancelled),
                ..TimerPatch::default()
            },
        )
        .await?;
    if !won {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Safe,
            "timer cancel lost a version race",
        ));
    }
    let row = txn
        .timers()
        .get(timer_id)
        .await?
        .expect("timer visible in its own transaction");
    stage_timer_event(
        txn,
        EventId::new(env.ids),
        "TimerCancelled",
        timer_stream(row.run_id),
        timer_payload(&row),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(row)
}

/// Claims a `Scheduled` timer for `owner` under `daemon_epoch`.
///
/// Returns `None` when the CAS loses (a cancel or a competing claim already
/// won). The claim fencing token is the post-claim version, which is unique
/// per claim because `version` only ever increments.
pub async fn claim(
    txn: &mut dyn KernelTxn,
    env: &SchedulerEnv<'_>,
    timer_id: TimerId,
    owner: &str,
    daemon_epoch: u64,
) -> errors::Result<Option<TimerRow>> {
    let row = txn
        .timers()
        .get(timer_id)
        .await?
        .ok_or_else(|| missing(timer_id))?;
    if row.state != TimerState::Scheduled {
        return Ok(None);
    }
    let won = txn
        .timers()
        .cas_transition(
            timer_id,
            TimerState::Scheduled,
            row.version,
            TimerPatch {
                state: Some(TimerState::Claimed),
                claim_owner: Some(owner.to_owned()),
                claim_fencing_token: Some(row.version + 1),
                claim_daemon_epoch: Some(daemon_epoch),
                ..TimerPatch::default()
            },
        )
        .await?;
    if !won {
        return Ok(None);
    }
    let claimed = txn
        .timers()
        .get(timer_id)
        .await?
        .expect("timer visible in its own transaction");
    stage_timer_event(
        txn,
        EventId::new(env.ids),
        "TimerClaimed",
        timer_stream(claimed.run_id),
        timer_payload(&claimed),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(Some(claimed))
}

/// Finds the next due timer the worker should dispatch: an unclaimed
/// `Scheduled` row it wins, a `Claimed` row already owned by this worker,
/// or a `Claimed` row left behind by a dead daemon epoch (reclaimed via
/// `Claimed -> Claimed` CAS, which bumps `version` and re-fences).
///
/// Returns the owned `Claimed` row ready to fire, or `None` when nothing
/// due can be claimed this pass.
pub async fn claim_next_due(
    txn: &mut dyn KernelTxn,
    env: &SchedulerEnv<'_>,
    now_ms: i64,
    owner: &str,
    daemon_epoch: u64,
) -> errors::Result<Option<TimerRow>> {
    for row in txn.timers().list_due(now_ms).await? {
        match row.state {
            TimerState::Claimed
                if row.claim_owner.as_deref() == Some(owner)
                    && row.claim_daemon_epoch == Some(daemon_epoch) =>
            {
                // We hold this claim already; fire is pending.
                return Ok(Some(row));
            }
            TimerState::Claimed => {
                // Stale claim from a dead epoch: re-fence to us.
                let won = txn
                    .timers()
                    .cas_transition(
                        row.timer_id,
                        TimerState::Claimed,
                        row.version,
                        TimerPatch {
                            state: Some(TimerState::Claimed),
                            claim_owner: Some(owner.to_owned()),
                            claim_fencing_token: Some(row.version + 1),
                            claim_daemon_epoch: Some(daemon_epoch),
                            ..TimerPatch::default()
                        },
                    )
                    .await?;
                if won {
                    return txn.timers().get(row.timer_id).await;
                }
            }
            TimerState::Scheduled => {
                if let Some(claimed) = claim(txn, env, row.timer_id, owner, daemon_epoch).await? {
                    return Ok(Some(claimed));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

/// Commits `Fired` for a `Claimed` row the worker still owns.
///
/// The CAS keys on the post-claim version, so after a crash/reclaim the
/// stale holder's commit loses and returns `false` — the timer can fire at
/// most once per ownership, and the deterministic [`fire_idempotency_key`]
/// dedupes the submitted command itself.
pub async fn mark_fired(
    txn: &mut dyn KernelTxn,
    env: &SchedulerEnv<'_>,
    claimed: &TimerRow,
) -> errors::Result<bool> {
    debug_assert!(claimed.state == TimerState::Claimed);
    let won = txn
        .timers()
        .cas_transition(
            claimed.timer_id,
            TimerState::Claimed,
            claimed.version,
            TimerPatch {
                state: Some(TimerState::Fired),
                ..TimerPatch::default()
            },
        )
        .await?;
    if !won {
        return Ok(false);
    }
    stage_timer_event(
        txn,
        EventId::new(env.ids),
        "TimerFired",
        timer_stream(claimed.run_id),
        timer_payload(claimed),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(true)
}

/// `run/<run-id>` for run-bound timers, `config/global` for daemon timers
/// (the catalog stream is run-scoped; run-less timers are out of catalog).
fn timer_stream(run_id: Option<RunId>) -> StreamKey {
    run_id.map(StreamKey::run).unwrap_or_else(StreamKey::config_global)
}

/// Canonical event payload: the schedule-shaped identity of the timer.
fn timer_payload(row: &TimerRow) -> Vec<u8> {
    contract::ScheduleTimer {
        timer_id: row.timer_id.to_string(),
        run_id: row.run_id.map(|id| id.to_string()).unwrap_or_default(),
        timer_kind: row.timer_kind.clone(),
        due_at_ms: row.due_at_ms,
        payload: row.payload.clone(),
    }
    .encode_to_vec()
}

fn missing(timer_id: TimerId) -> KernelError {
    KernelError::new(
        ErrorCode::NotFound,
        RetryClass::Never,
        format!("timer {timer_id} does not exist"),
    )
}
