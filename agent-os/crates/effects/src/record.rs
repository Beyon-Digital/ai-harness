//! `EffectRecord` state machine: the legal transition table.
//!
//! The lifecycle is fixed by `specs/effect-coordinator.md`:
//!
//! ```text
//! Prepared -> Claimed -> Dispatched -> Acknowledged -> Committed
//!                        |               |
//!                        +-> Failed      +-> Failed
//!                        +-> Cancelled
//!                        +-> Unknown
//! ```
//!
//! `Unknown` is resolved only by the explicit `ResolveUnknownEffect` command:
//! it settles to `Committed`/`Failed`, or returns to `Prepared` for a
//! duplicate-risk-accepted retry of the same operation identity.

use domain::effect::EffectState;

/// Returns whether `from -> to` is a legal effect transition.
///
/// Reclaim of an expired `Claimed` lease is expressed as
/// `Claimed -> Claimed`; re-dispatch during recovery re-enters through
/// `Dispatched -> Dispatched` (new fencing token, same operation identity).
/// `Unknown` resolves only through `ResolveUnknownEffect`.
pub const fn can_transition(from: EffectState, to: EffectState) -> bool {
    matches!(
        (from, to),
        (EffectState::Prepared, EffectState::Claimed)
            | (EffectState::Prepared, EffectState::Cancelled)
            | (EffectState::Claimed, EffectState::Claimed)
            | (EffectState::Claimed, EffectState::Dispatched)
            | (EffectState::Claimed, EffectState::Cancelled)
            | (EffectState::Dispatched, EffectState::Dispatched)
            | (EffectState::Dispatched, EffectState::Acknowledged)
            | (EffectState::Dispatched, EffectState::Failed)
            | (EffectState::Dispatched, EffectState::Cancelled)
            | (EffectState::Dispatched, EffectState::Unknown)
            | (EffectState::Acknowledged, EffectState::Committed)
            | (EffectState::Acknowledged, EffectState::Failed)
            | (EffectState::Unknown, EffectState::Committed)
            | (EffectState::Unknown, EffectState::Failed)
            | (EffectState::Unknown, EffectState::Prepared)
    )
}

/// Returns whether `state` is terminal with no outgoing transitions.
pub const fn is_terminal(state: EffectState) -> bool {
    matches!(
        state,
        EffectState::Committed | EffectState::Failed | EffectState::Cancelled
    )
}

/// Returns whether `state` still admits executor work or reconciliation.
pub const fn is_in_flight(state: EffectState) -> bool {
    !is_terminal(state) && !matches!(state, EffectState::Unspecified)
}

#[cfg(test)]
mod tests {
    use domain::effect::EffectState;

    use super::{can_transition, is_terminal};

    #[test]
    fn happy_path_chain_is_legal() {
        for (from, to) in [
            (EffectState::Prepared, EffectState::Claimed),
            (EffectState::Claimed, EffectState::Dispatched),
            (EffectState::Dispatched, EffectState::Acknowledged),
            (EffectState::Acknowledged, EffectState::Committed),
        ] {
            assert!(can_transition(from, to), "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn terminal_states_have_no_outgoing_transitions() {
        for from in [
            EffectState::Committed,
            EffectState::Failed,
            EffectState::Cancelled,
        ] {
            for to in [
                EffectState::Prepared,
                EffectState::Claimed,
                EffectState::Dispatched,
                EffectState::Acknowledged,
                EffectState::Committed,
                EffectState::Failed,
                EffectState::Cancelled,
                EffectState::Unknown,
            ] {
                assert!(!can_transition(from, to), "{from:?} -> {to:?}");
                assert!(is_terminal(from));
            }
        }
    }

    #[test]
    fn skipped_and_reversed_transitions_are_illegal() {
        for (from, to) in [
            (EffectState::Prepared, EffectState::Dispatched),
            (EffectState::Prepared, EffectState::Committed),
            (EffectState::Claimed, EffectState::Acknowledged),
            (EffectState::Dispatched, EffectState::Prepared),
            (EffectState::Acknowledged, EffectState::Dispatched),
            (EffectState::Committed, EffectState::Unknown),
        ] {
            assert!(!can_transition(from, to), "{from:?} -> {to:?}");
        }
    }
}
