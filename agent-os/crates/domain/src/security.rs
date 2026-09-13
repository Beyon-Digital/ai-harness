//! Security mirror enums and immutable value types.

pub use crate::run::{UnknownEnumValue, UnknownStateValue};

use crate::run::{mirror_enum, state_enum};

mirror_enum! {
    /// Sensitivity of an event, mirroring `contract::Sensitivity`.
    SensitivityClass, "SensitivityClass", {
        Unspecified = 0,
        Public = 1,
        Internal = 2,
        Confidential = 3,
        Secret = 4,
    }
}

mirror_enum! {
    /// Retention class of an event, mirroring `contract::RetentionClass`.
    RetentionClass, "RetentionClass", {
        Unspecified = 0,
        Ephemeral = 1,
        Standard = 2,
        Audit = 3,
    }
}

state_enum! {
    /// Trust state of a registered adapter.
    TrustState, "TrustState", {
        Trusted = 1 => "trusted",
        Untrusted = 2 => "untrusted",
    }
}

state_enum! {
    /// Conformance state of a registered adapter.
    ConformanceState, "ConformanceState", {
        Untested = 1 => "untested",
        Passed = 2 => "passed",
        Failed = 3 => "failed",
    }
}

state_enum! {
    /// State of an approval request.
    ApprovalState, "ApprovalState", {
        Pending = 1 => "pending",
        Approved = 2 => "approved",
        Denied = 3 => "denied",
        Expired = 4 => "expired",
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalState, ConformanceState, RetentionClass, SensitivityClass, TrustState};

    #[test]
    fn wire_mapping_matches_the_persisted_state_order() {
        assert_eq!(SensitivityClass::Confidential.to_wire(), 3);
        assert_eq!(SensitivityClass::Secret.to_wire(), 4);
        assert_eq!(RetentionClass::Standard.to_wire(), 2);
        assert_eq!(RetentionClass::Audit.to_wire(), 3);
        assert_eq!(TrustState::Untrusted.to_wire(), 2);
        assert_eq!(ConformanceState::Passed.to_wire(), 2);
        assert_eq!(ApprovalState::Expired.to_wire(), 4);
    }

    #[test]
    fn unknown_values_are_not_guessed() {
        assert!(SensitivityClass::from_wire(5).is_err());
        assert!(RetentionClass::from_wire(4).is_err());
        assert!(TrustState::from_wire(0).is_err());
        assert!(TrustState::from_wire(3).is_err());
        assert!(ConformanceState::from_wire(4).is_err());
        assert!(ApprovalState::from_wire(0).is_err());
        assert!(ApprovalState::from_wire(5).is_err());
    }

    #[test]
    fn persisted_state_strings_round_trip() {
        assert_eq!(TrustState::Trusted.as_str(), "trusted");
        assert_eq!(
            TrustState::from_state_str("untrusted"),
            Ok(TrustState::Untrusted)
        );
        assert_eq!(ConformanceState::Failed.as_str(), "failed");
        assert_eq!(
            ApprovalState::from_state_str("pending"),
            Ok(ApprovalState::Pending)
        );
        assert!(TrustState::from_state_str("trusted ").is_err());
        assert!(ApprovalState::from_state_str("superseded").is_err());
    }
}
