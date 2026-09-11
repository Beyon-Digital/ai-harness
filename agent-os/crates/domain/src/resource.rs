//! Resource mirror enums and immutable value types.

pub use crate::run::UnknownEnumValue;

use crate::run::mirror_enum;

mirror_enum! {
    /// Workspace access mode held by a lease, mirroring `contract::WorkspaceAccessMode`.
    WorkspaceAccessMode, "WorkspaceAccessMode", {
        Unspecified = 0,
        ReadOnly = 1,
        ExclusiveWrite = 2,
        IsolatedFork = 3,
        SharedCoordinatedWrite = 4,
    }
}

mirror_enum! {
    /// Enforcement lifecycle of a workspace lease.
    LeaseEnforcementState, "LeaseEnforcementState", {
        Unspecified = 0,
        Active = 1,
        Revoked = 2,
    }
}

mirror_enum! {
    /// Lifecycle state of a scheduled timer.
    TimerState, "TimerState", {
        Unspecified = 0,
        Scheduled = 1,
        Claimed = 2,
        Fired = 3,
        Cancelled = 4,
    }
}

mirror_enum! {
    /// Lifecycle state of a resource reservation.
    ReservationState, "ReservationState", {
        Unspecified = 0,
        Reserved = 1,
        Allocated = 2,
        Released = 3,
        Expired = 4,
        Unknown = 5,
    }
}

mirror_enum! {
    /// Condition under which a run dependency is satisfied, mirroring `contract::DependencyCondition`.
    DependencyCondition, "DependencyCondition", {
        Unspecified = 0,
        CompletedSuccessfully = 1,
        AnyTerminal = 2,
        CompletedOrCancelled = 3,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DependencyCondition, LeaseEnforcementState, ReservationState, TimerState,
        WorkspaceAccessMode,
    };

    #[test]
    fn wire_mapping_matches_the_persisted_state_order() {
        assert_eq!(WorkspaceAccessMode::ExclusiveWrite.to_wire(), 2);
        assert_eq!(LeaseEnforcementState::Revoked.to_wire(), 2);
        assert_eq!(TimerState::Fired.to_wire(), 3);
        assert_eq!(ReservationState::Unknown.to_wire(), 5);
        assert_eq!(DependencyCondition::CompletedOrCancelled.to_wire(), 3);
    }

    #[test]
    fn unknown_values_are_not_guessed() {
        assert!(WorkspaceAccessMode::from_wire(5).is_err());
        assert!(LeaseEnforcementState::from_wire(3).is_err());
        assert!(TimerState::from_wire(5).is_err());
        assert!(ReservationState::from_wire(6).is_err());
        assert!(DependencyCondition::from_wire(4).is_err());
    }
}
