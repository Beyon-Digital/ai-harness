//! Resource mirror enums and immutable value types.

pub use crate::run::{UnknownEnumValue, UnknownStateValue};

use crate::run::{mirror_enum, state_enum};

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

state_enum! {
    /// Enforcement lifecycle of a workspace lease.
    LeaseEnforcementState, "LeaseEnforcementState", {
        Active = 1 => "active",
        Revoked = 2 => "revoked",
    }
}

state_enum! {
    /// Lifecycle state of a scheduled timer.
    TimerState, "TimerState", {
        Scheduled = 1 => "scheduled",
        Claimed = 2 => "claimed",
        Fired = 3 => "fired",
        Cancelled = 4 => "cancelled",
    }
}

state_enum! {
    /// Lifecycle state of a resource reservation.
    ReservationState, "ReservationState", {
        Reserved = 1 => "reserved",
        Allocated = 2 => "allocated",
        Released = 3 => "released",
        Expired = 4 => "expired",
        Unknown = 5 => "unknown",
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
        assert!(LeaseEnforcementState::from_wire(0).is_err());
        assert!(LeaseEnforcementState::from_wire(3).is_err());
        assert!(TimerState::from_wire(0).is_err());
        assert!(TimerState::from_wire(5).is_err());
        assert!(ReservationState::from_wire(6).is_err());
        assert!(DependencyCondition::from_wire(4).is_err());
    }

    #[test]
    fn persisted_state_strings_round_trip() {
        assert_eq!(LeaseEnforcementState::Active.as_str(), "active");
        assert_eq!(
            LeaseEnforcementState::from_state_str("revoked"),
            Ok(LeaseEnforcementState::Revoked)
        );
        assert_eq!(TimerState::Scheduled.as_str(), "scheduled");
        assert_eq!(TimerState::from_state_str("fired"), Ok(TimerState::Fired));
        assert_eq!(
            ReservationState::from_state_str("allocated"),
            Ok(ReservationState::Allocated)
        );
        assert!(TimerState::from_state_str("Scheduled").is_err());
        assert!(ReservationState::from_state_str("reserved ").is_err());
    }
}
