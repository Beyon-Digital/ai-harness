//! Typed effect contract: the kernel-effective side-effect policy axes.
//!
//! [`EffectContract`] is the resolved contract persisted on every
//! `EffectRecord` (spec: `effect-coordinator.md`). Each axis is independent:
//! class, idempotency, reconciliation, cancellation, and an optional
//! compensation capability.

use std::fmt;
use std::str::FromStr;

use domain::effect::{EffectClass, IdempotencySemantics, ReconciliationSemantics};
use domain::generated::contract;
use domain::run::UnknownStateValue;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// Cancellation vocabulary persisted in `effects.cancellation_semantics`.
///
/// The stored literals are the canonical lowercase tokens below; the contract
/// vocabulary (`BeforeDispatch | Cooperative | ProviderSpecific |
/// Unsupported`) is fixed by `specs/effect-coordinator.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancellationSemantics {
    /// The kernel can refuse the dispatch before the adapter is called.
    BeforeDispatch,
    /// The adapter accepts a cooperative cancellation request.
    Cooperative,
    /// Cancellation rides a provider-specific mechanism.
    ProviderSpecific,
    /// Cancellation is not possible.
    Unsupported,
}

impl CancellationSemantics {
    /// Returns the exact persisted token for this semantics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeDispatch => "before_dispatch",
            Self::Cooperative => "cooperative",
            Self::ProviderSpecific => "provider_specific",
            Self::Unsupported => "unsupported",
        }
    }

    /// Parses an exact persisted token, rejecting unknown values.
    pub fn from_state_str(value: &str) -> Result<Self, UnknownStateValue> {
        match value {
            "before_dispatch" => Ok(Self::BeforeDispatch),
            "cooperative" => Ok(Self::Cooperative),
            "provider_specific" => Ok(Self::ProviderSpecific),
            "unsupported" => Ok(Self::Unsupported),
            _ => Err(UnknownStateValue {
                value: value.to_owned(),
                enum_name: "CancellationSemantics",
            }),
        }
    }
}

impl fmt::Display for CancellationSemantics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CancellationSemantics {
    type Err = UnknownStateValue;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_state_str(text)
    }
}

/// The kernel-effective effect contract: the least-safe semantics every
/// evidence source supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectContract {
    /// Side-effect surface classification.
    pub effect_class: EffectClass,
    /// Idempotent-replay semantics.
    pub idempotency: IdempotencySemantics,
    /// Outcome-reconciliation semantics.
    pub reconciliation: ReconciliationSemantics,
    /// Cancellation semantics.
    pub cancellation: CancellationSemantics,
    /// Compensation capability when a trusted source declares one.
    pub compensation_capability: Option<String>,
}

impl EffectContract {
    /// The contract an unknown or generated tool resolves to
    /// (`Opaque + Unknown + Unknown + Unsupported`).
    pub fn unknown() -> Self {
        Self {
            effect_class: EffectClass::Opaque,
            idempotency: IdempotencySemantics::UnknownIdempotency,
            reconciliation: ReconciliationSemantics::UnknownReconciliation,
            cancellation: CancellationSemantics::Unsupported,
            compensation_capability: None,
        }
    }

    /// True when the adapter contract supports querying the outcome of a
    /// dispatched operation by its stable operation identity.
    pub fn is_reconcilable(&self) -> bool {
        matches!(
            self.reconciliation,
            ReconciliationSemantics::StatusLookup
                | ReconciliationSemantics::ResultLookup
                | ReconciliationSemantics::DeterministicInspection
        )
    }

    /// True when redispatching the same operation identity cannot double-apply
    /// the effect: natural idempotency or a provider-honoured idempotency key.
    pub fn is_safely_redispatchable(&self) -> bool {
        matches!(
            self.idempotency,
            IdempotencySemantics::NaturallyIdempotent
                | IdempotencySemantics::IdempotencyKeySupported
        )
    }

    /// True when a never-dispatched effect can be cancelled before dispatch.
    pub fn can_cancel_before_dispatch(&self) -> bool {
        self.cancellation == CancellationSemantics::BeforeDispatch
    }

    /// Projects the typed contract onto the wire `EffectContract` message.
    pub fn to_contract(&self) -> contract::EffectContract {
        contract::EffectContract {
            effect_class: self.effect_class.to_wire(),
            idempotency: self.idempotency.to_wire(),
            reconciliation: self.reconciliation.to_wire(),
            cancellation_semantics: self.cancellation.as_str().to_owned(),
            compensation_capability: self.compensation_capability.clone().unwrap_or_default(),
        }
    }

    /// Decodes a wire `EffectContract`, rejecting non-canonical values instead
    /// of guessing them.
    pub fn from_contract(raw: &contract::EffectContract) -> errors::Result<Self> {
        Ok(Self {
            effect_class: EffectClass::from_wire(raw.effect_class).map_err(decode_err)?,
            idempotency: IdempotencySemantics::from_wire(raw.idempotency).map_err(decode_err)?,
            reconciliation: ReconciliationSemantics::from_wire(raw.reconciliation)
                .map_err(decode_err)?,
            cancellation: CancellationSemantics::from_state_str(&raw.cancellation_semantics)
                .map_err(decode_err)?,
            compensation_capability: match raw.compensation_capability.as_str() {
                "" => None,
                capability => Some(capability.to_owned()),
            },
        })
    }
}

fn decode_err(detail: impl fmt::Display) -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        format!("effect contract field is not canonical: {detail}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{CancellationSemantics, EffectContract};
    use domain::effect::{EffectClass, IdempotencySemantics, ReconciliationSemantics};

    #[test]
    fn unknown_default_is_the_conservative_floor() {
        let contract = EffectContract::unknown();
        assert_eq!(contract.effect_class, EffectClass::Opaque);
        assert_eq!(
            contract.idempotency,
            IdempotencySemantics::UnknownIdempotency
        );
        assert_eq!(
            contract.reconciliation,
            ReconciliationSemantics::UnknownReconciliation
        );
        assert_eq!(contract.cancellation, CancellationSemantics::Unsupported);
        assert_eq!(contract.compensation_capability, None);
        assert!(!contract.is_reconcilable());
        assert!(!contract.is_safely_redispatchable());
        assert!(!contract.can_cancel_before_dispatch());
    }

    #[test]
    fn cancellation_tokens_round_trip() {
        for semantics in [
            CancellationSemantics::BeforeDispatch,
            CancellationSemantics::Cooperative,
            CancellationSemantics::ProviderSpecific,
            CancellationSemantics::Unsupported,
        ] {
            assert_eq!(
                CancellationSemantics::from_state_str(semantics.as_str()),
                Ok(semantics)
            );
        }
        assert!(CancellationSemantics::from_state_str("BEFORE_DISPATCH").is_err());
        assert!(CancellationSemantics::from_state_str("").is_err());
    }

    #[test]
    fn contract_wire_round_trip() {
        let contract = EffectContract {
            effect_class: EffectClass::ExternalMutation,
            idempotency: IdempotencySemantics::IdempotencyKeySupported,
            reconciliation: ReconciliationSemantics::ResultLookup,
            cancellation: CancellationSemantics::Cooperative,
            compensation_capability: Some("compensate".to_owned()),
        };
        let decoded = EffectContract::from_contract(&contract.to_contract()).expect("decodes");
        assert_eq!(decoded, contract);
        assert!(decoded.is_reconcilable());
        assert!(decoded.is_safely_redispatchable());
    }
}
