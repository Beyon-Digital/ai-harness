//! Effect contracts, state, leases, and reconciliation coordination.
#![forbid(unsafe_code)]

pub mod contract;
pub mod coordinator;
pub mod env;
pub mod executor;
pub mod policy;
pub mod reconcile;
pub mod record;
mod stage;

pub use contract::{CancellationSemantics, EffectContract};
pub use coordinator::{AdapterBinding, PrepareOutcome, PrepareRequest, prepare_effect};
pub use env::EffectEnv;
pub use executor::{
    ClaimOutcome, EFFECT_LEASE_MS, ExecutorRef, acknowledge, cancel, claim, commit, fail,
    mark_dispatched, mark_unknown,
};
pub use policy::{DeclaredSemantics, ResolveInputs, resolve};
pub use reconcile::{
    EffectExecutor, ExecutionOutcome, ExecutionRequest, ObservedOutcome, ReconcilePlan, Resolution,
    apply_observed, apply_resolution, reconcile_plan,
};
pub use record::{can_transition, is_in_flight, is_terminal};
