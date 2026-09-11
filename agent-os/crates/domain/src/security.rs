//! Security mirror enums and immutable value types.

pub use crate::run::UnknownEnumValue;

use crate::run::mirror_enum;

mirror_enum! {
    /// Sensitivity of an event, mirroring `contract::Sensitivity`.
    SensitivityClass, "SensitivityClass", {
        Unspecified = 0,
        Public = 1,
        Internal = 2,
        Private = 3,
        Secret = 4,
    }
}

mirror_enum! {
    /// Retention class of an event, mirroring `contract::RetentionClass`.
    RetentionClass, "RetentionClass", {
        Unspecified = 0,
        Ephemeral = 1,
        Session = 2,
        Audit = 3,
        Durable = 4,
    }
}

mirror_enum! {
    /// Trust state of a registered adapter.
    TrustState, "TrustState", {
        Unspecified = 0,
        Trusted = 1,
        Untrusted = 2,
    }
}

mirror_enum! {
    /// Conformance state of a registered adapter.
    ConformanceState, "ConformanceState", {
        Unspecified = 0,
        Untested = 1,
        Passed = 2,
        Failed = 3,
    }
}

mirror_enum! {
    /// State of an approval request.
    ApprovalState, "ApprovalState", {
        Unspecified = 0,
        Pending = 1,
        Approved = 2,
        Denied = 3,
        Expired = 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalState, ConformanceState, SensitivityClass, TrustState};

    #[test]
    fn wire_mapping_matches_the_persisted_state_order() {
        assert_eq!(SensitivityClass::Secret.to_wire(), 4);
        assert_eq!(TrustState::Untrusted.to_wire(), 2);
        assert_eq!(ConformanceState::Passed.to_wire(), 2);
        assert_eq!(ApprovalState::Expired.to_wire(), 4);
    }

    #[test]
    fn unknown_values_are_not_guessed() {
        assert!(SensitivityClass::from_wire(5).is_err());
        assert!(TrustState::from_wire(3).is_err());
        assert!(ConformanceState::from_wire(4).is_err());
        assert!(ApprovalState::from_wire(5).is_err());
    }
}
