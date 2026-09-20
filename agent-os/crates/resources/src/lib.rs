//! Durable resource reservations and budget delegation.
//!
//! Reservations are durable rows (`reserved -> allocated -> released`, with
//! `expired`/`unknown` exceptional), never transient counters. A child may
//! reference a parent reservation; the delegation check runs inside the
//! inserting transaction so racing children cannot exceed the delegated
//! budget (`sum(active children) <= parent.amount`). `unknown` children
//! count toward usage — an uncertain external allocation is never silently
//! released (resources.md).
#![forbid(unsafe_code)]

mod stage;

use domain::generated::contract;
use domain::ids::{EventId, ReservationId, RunId};
use domain::provider::IdProvider;
use domain::resource::ReservationState;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::{NewReservation, ReservationPatch, ReservationRow};
use prost::Message;
use stage::stage_resource_event;
use std::str::FromStr;

/// Typed MVP budget units (resources.md "MVP budget types").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceUnit {
    /// Child runs a parent run may create.
    ChildRunSlots,
    /// Sandboxes the run may hold.
    SandboxSlots,
    /// Model tokens (structure only; no real adapter yet).
    ModelTokens,
    /// Model spend in microunits.
    ModelCostMicrounits,
    /// Wall-clock budget in milliseconds.
    WallClockMs,
    /// Disk budget in bytes.
    DiskBytes,
}

impl ResourceUnit {
    /// Canonical unit string persisted on the row.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChildRunSlots => "child_run_slots",
            Self::SandboxSlots => "sandbox_slots",
            Self::ModelTokens => "model_tokens",
            Self::ModelCostMicrounits => "model_cost_microunits",
            Self::WallClockMs => "wall_clock_ms",
            Self::DiskBytes => "disk_bytes",
        }
    }
}

impl FromStr for ResourceUnit {
    type Err = KernelError;
    fn from_str(value: &str) -> errors::Result<Self> {
        Ok(match value {
            "child_run_slots" => Self::ChildRunSlots,
            "sandbox_slots" => Self::SandboxSlots,
            "model_tokens" => Self::ModelTokens,
            "model_cost_microunits" => Self::ModelCostMicrounits,
            "wall_clock_ms" => Self::WallClockMs,
            "disk_bytes" => Self::DiskBytes,
            other => {
                return Err(KernelError::new(
                    ErrorCode::InvalidArgument,
                    RetryClass::Never,
                    format!("unknown resource unit {other:?}"),
                ));
            }
        })
    }
}

/// Clock/id environment shared by reservation mutations.
pub struct ResourceEnv<'a> {
    /// Identifier source for reservations and outbox events.
    pub ids: &'a dyn IdProvider,
    /// Clock for persisted timestamps.
    pub clock: &'a dyn Clock,
    /// Correlation id propagated onto staged events.
    pub correlation_id: Option<String>,
    /// Causing event, when the mutation was event-driven.
    pub causation_id: Option<EventId>,
}

/// Fields of a reservation request.
pub struct ReserveRequest {
    /// Caller-supplied id; minted when absent. Identical replays return the
    /// stored row, divergent reuse conflicts.
    pub reservation_id: Option<ReservationId>,
    /// Owning run.
    pub run_id: RunId,
    /// Budget unit.
    pub unit: ResourceUnit,
    /// Amount of `unit` reserved; must be positive.
    pub amount: i64,
    /// Parent reservation this child delegates from.
    pub parent: Option<ReservationId>,
    /// Fencing token identifying the external owner of the reserved
    /// resource (0 for kernel-internal reservations).
    pub fencing_token: u64,
}

/// States that still hold budget. `Unknown` counts: an uncertain external
/// allocation may still be consuming.
const ACTIVE: [ReservationState; 3] = [
    ReservationState::Reserved,
    ReservationState::Allocated,
    ReservationState::Unknown,
];

/// Reserves `amount` of `unit` for `run_id` inside the caller's
/// transaction.
///
/// With `parent`, the delegated budget is enforced transactionally: the sum
/// of the parent's active (`reserved|allocated|unknown`) same-unit children
/// plus `amount` must not exceed `parent.amount` — `ResourceExhausted`
/// otherwise. Cross-unit or missing parents are `FailedPrecondition`.
pub async fn reserve(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    request: ReserveRequest,
) -> errors::Result<ReservationRow> {
    if request.amount <= 0 {
        return Err(KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "reservation amount must be positive",
        ));
    }
    let reservation_id = request
        .reservation_id
        .unwrap_or_else(|| ReservationId::new(env.ids));
    if let Some(existing) = txn.resources().get(reservation_id).await? {
        let identical = existing.run_id == request.run_id
            && existing.unit == request.unit.as_str()
            && existing.amount == request.amount
            && existing.parent_reservation_id == request.parent;
        if identical {
            return Ok(existing);
        }
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            "reservation_id reused with different fields",
        ));
    }
    if let Some(parent_id) = request.parent {
        check_parent_budget(txn, parent_id, request.unit, request.amount).await?;
    }
    let now_ms = env.clock.now_unix_ms();
    txn.resources()
        .insert(NewReservation {
            reservation_id,
            run_id: request.run_id,
            resource_type: request.unit.as_str().to_owned(),
            state: ReservationState::Reserved,
            amount: request.amount,
            unit: request.unit.as_str().to_owned(),
            fencing_token: request.fencing_token,
            parent_reservation_id: request.parent,
            created_at_ms: now_ms,
        })
        .await?;
    let row = txn
        .resources()
        .get(reservation_id)
        .await?
        .expect("reservation visible in its own transaction");
    stage_resource_event(
        txn,
        EventId::new(env.ids),
        "ResourceReserved",
        row.run_id,
        reservation_payload(&row),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(row)
}

/// Transactional delegation check: active descendant totals plus `amount`
/// must fit the parent's budget for the same unit.
async fn check_parent_budget(
    txn: &mut dyn KernelTxn,
    parent_id: ReservationId,
    unit: ResourceUnit,
    amount: i64,
) -> errors::Result<()> {
    let parent = txn
        .resources()
        .get(parent_id)
        .await?
        .ok_or_else(|| missing(parent_id))?;
    if !ACTIVE.contains(&parent.state) {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!("parent reservation is {}", parent.state.as_str()),
        ));
    }
    if parent.unit != unit.as_str() {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "child unit differs from the parent reservation's unit",
        ));
    }
    let delegated: i64 = txn
        .resources()
        .list_children(parent_id)
        .await?
        .iter()
        .filter(|child| ACTIVE.contains(&child.state) && child.unit == unit.as_str())
        .map(|child| child.amount)
        .sum();
    if delegated + amount > parent.amount {
        return Err(KernelError::new(
            ErrorCode::ResourceExhausted,
            RetryClass::Safe,
            format!(
                "delegated {delegated} + {amount} exceeds parent budget {}",
                parent.amount
            ),
        ));
    }
    Ok(())
}

/// `reserved -> allocated`, fenced by the row's external ownership token.
///
/// A mismatched `fencing_token` is `FailedPrecondition` — a stale owner may
/// not allocate a re-fenced reservation.
pub async fn allocate(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    id: ReservationId,
    fencing_token: u64,
) -> errors::Result<ReservationRow> {
    let row = txn.resources().get(id).await?.ok_or_else(|| missing(id))?;
    verify_token(&row, fencing_token)?;
    let won = txn
        .resources()
        .cas_transition(
            id,
            ReservationState::Reserved,
            ReservationPatch {
                state: Some(ReservationState::Allocated),
                ..ReservationPatch::default()
            },
        )
        .await?;
    if !won {
        return Err(state_conflict(&row));
    }
    stage_and_fetch(txn, env, id, "ResourceAllocated").await
}

/// `allocated -> released`, fenced by the row's external ownership token.
pub async fn release(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    id: ReservationId,
    fencing_token: u64,
) -> errors::Result<ReservationRow> {
    let row = txn.resources().get(id).await?.ok_or_else(|| missing(id))?;
    verify_token(&row, fencing_token)?;
    let won = txn
        .resources()
        .cas_transition(
            id,
            ReservationState::Allocated,
            ReservationPatch {
                state: Some(ReservationState::Released),
                ..ReservationPatch::default()
            },
        )
        .await?;
    if !won {
        return Err(state_conflict(&row));
    }
    stage_and_fetch(txn, env, id, "ResourceReleased").await
}

/// `allocated|reserved -> unknown` for an allocation whose external outcome
/// is uncertain. Unknown rows still consume budget (they are active), so a
/// possibly-leaked external allocation cannot be silently re-sold.
pub async fn mark_unknown(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    id: ReservationId,
) -> errors::Result<ReservationRow> {
    let row = txn.resources().get(id).await?.ok_or_else(|| missing(id))?;
    if !matches!(
        row.state,
        ReservationState::Allocated | ReservationState::Reserved
    ) {
        return Err(state_conflict(&row));
    }
    let won = txn
        .resources()
        .cas_transition(
            id,
            row.state,
            ReservationPatch {
                state: Some(ReservationState::Unknown),
                ..ReservationPatch::default()
            },
        )
        .await?;
    if !won {
        return Err(state_conflict(&row));
    }
    stage_and_fetch(txn, env, id, "ResourceUnknown").await
}

/// Re-fences an `unknown` reservation to a new external owner and retries
/// `unknown -> allocated`, recovering an allocation whose previous owner
/// left it uncertain. The row's `fencing_token` is replaced, so a stale
/// owner can no longer act on it.
pub async fn recover_unknown(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    id: ReservationId,
    new_fencing_token: u64,
) -> errors::Result<ReservationRow> {
    let row = txn.resources().get(id).await?.ok_or_else(|| missing(id))?;
    let won = txn
        .resources()
        .cas_transition(
            id,
            ReservationState::Unknown,
            ReservationPatch {
                state: Some(ReservationState::Allocated),
                fencing_token: Some(new_fencing_token),
            },
        )
        .await?;
    if !won {
        return Err(state_conflict(&row));
    }
    stage_and_fetch(txn, env, id, "ResourceAllocated").await
}

/// Canonical event payload for a reservation row.
fn reservation_payload(row: &ReservationRow) -> Vec<u8> {
    contract::ResourceReservation {
        reservation_id: row.reservation_id.to_string(),
        run_id: row.run_id.to_string(),
        resource_type: row.resource_type.clone(),
        state: row.state.as_str().to_owned(),
        amount: row.amount,
        unit: row.unit.clone(),
        fencing_token: row.fencing_token,
    }
    .encode_to_vec()
}

async fn stage_and_fetch(
    txn: &mut dyn KernelTxn,
    env: &ResourceEnv<'_>,
    id: ReservationId,
    event_type: &'static str,
) -> errors::Result<ReservationRow> {
    let row = txn
        .resources()
        .get(id)
        .await?
        .expect("reservation visible in its own transaction");
    stage_resource_event(
        txn,
        EventId::new(env.ids),
        event_type,
        row.run_id,
        reservation_payload(&row),
        env.correlation_id.clone(),
        env.causation_id,
    )
    .await?;
    Ok(row)
}

fn verify_token(row: &ReservationRow, fencing_token: u64) -> errors::Result<()> {
    if row.fencing_token != fencing_token {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "fencing token does not own this reservation",
        ));
    }
    Ok(())
}

fn missing(id: ReservationId) -> KernelError {
    KernelError::new(
        ErrorCode::NotFound,
        RetryClass::Never,
        format!("reservation {id} does not exist"),
    )
}

fn state_conflict(row: &ReservationRow) -> KernelError {
    KernelError::new(
        ErrorCode::Conflict,
        RetryClass::Safe,
        format!("reservation lost a race from state {}", row.state.as_str()),
    )
}
