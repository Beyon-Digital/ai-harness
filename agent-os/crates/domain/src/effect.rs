//! Effect mirror enums and immutable value types.

pub use crate::run::UnknownEnumValue;

use crate::run::mirror_enum;

mirror_enum! {
    /// Class of side effect, mirroring `contract::EffectClass`.
    EffectClass, "EffectClass", {
        Unspecified = 0,
        ReadOnly = 1,
        LocalMutation = 2,
        ExternalMutation = 3,
        Opaque = 4,
    }
}

mirror_enum! {
    /// Idempotency semantics of an effect, mirroring `contract::IdempotencySemantics`.
    IdempotencySemantics, "IdempotencySemantics", {
        Unspecified = 0,
        NaturallyIdempotent = 1,
        IdempotencyKeySupported = 2,
        NotIdempotent = 3,
        UnknownIdempotency = 4,
    }
}

mirror_enum! {
    /// Reconciliation semantics of an effect, mirroring `contract::ReconciliationSemantics`.
    ReconciliationSemantics, "ReconciliationSemantics", {
        Unspecified = 0,
        StatusLookup = 1,
        ResultLookup = 2,
        DeterministicInspection = 3,
        Impossible = 4,
        UnknownReconciliation = 5,
    }
}

mirror_enum! {
    /// Execution state of an effect, mirroring `contract::EffectState`.
    EffectState, "EffectState", {
        Unspecified = 0,
        Prepared = 1,
        Claimed = 2,
        Dispatched = 3,
        Acknowledged = 4,
        Committed = 5,
        Failed = 6,
        Cancelled = 7,
        Unknown = 8,
    }
}

#[cfg(test)]
mod tests {
    use super::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};

    #[test]
    fn gc8_renamed_values_map_to_their_original_numbers() {
        assert_eq!(EffectClass::ReadOnly.to_wire(), 1);
        assert_eq!(EffectState::Failed.to_wire(), 6);
        assert_eq!(EffectState::Cancelled.to_wire(), 7);
    }

    #[test]
    fn unknown_values_are_not_guessed() {
        assert!(EffectClass::from_wire(5).is_err());
        assert!(EffectState::from_wire(9).is_err());
        assert!(IdempotencySemantics::from_wire(5).is_err());
        assert!(ReconciliationSemantics::from_wire(6).is_err());
    }
}
