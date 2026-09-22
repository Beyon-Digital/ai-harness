//! Conservative effect-policy resolver.
//!
//! An adapter or generated tool's declaration is a *claim*, not evidence.
//! [`resolve`] computes the kernel-effective [`EffectContract`] as the
//! conservative intersection of kernel-known port semantics, the adapter's
//! self-declaration, its trust tier, its conformance evidence, and explicit
//! operator policy (spec: `effect-coordinator.md`, "Effective contract").
//!
//! Resolution rules:
//!
//! - every axis resolves independently to the **least safe** value among the
//!   contributing evidence sources;
//! - a claim contributes only when the adapter is `Trusted` **and** its
//!   conformance state is `Passed`;
//! - an explicit operator policy always contributes;
//! - an axis with no contributing value resolves to its most conservative
//!   default (`Opaque` / `Unknown` / `Unknown` / `Unsupported`), so absent
//!   evidence can never silently promote a contract.

use domain::effect::{EffectClass, IdempotencySemantics, ReconciliationSemantics};
use domain::security::{ConformanceState, TrustState};

use crate::contract::{CancellationSemantics, EffectContract};

/// Semantics declared by one evidence source; every axis is independently
/// optional, so a partial declaration covers only the axes it names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredSemantics {
    /// Claimed or known effect class.
    pub effect_class: Option<EffectClass>,
    /// Claimed or known idempotency semantics.
    pub idempotency: Option<IdempotencySemantics>,
    /// Claimed or known reconciliation semantics.
    pub reconciliation: Option<ReconciliationSemantics>,
    /// Claimed or known cancellation semantics.
    pub cancellation: Option<CancellationSemantics>,
    /// Claimed or known compensation capability.
    pub compensation_capability: Option<String>,
}

/// The inputs the resolver intersects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveInputs {
    /// Kernel-known semantics of the bound port; always contributes.
    pub kernel: Option<DeclaredSemantics>,
    /// The adapter/extension's self-declared semantics; contributes only when
    /// the adapter is trusted and its conformance evidence has passed.
    pub claim: Option<DeclaredSemantics>,
    /// Trust state of the bound adapter registration.
    pub trust: TrustState,
    /// Conformance state of the bound adapter registration.
    pub conformance: ConformanceState,
    /// Explicit operator/admin policy constraint; always contributes.
    pub policy: Option<DeclaredSemantics>,
}

impl Default for ResolveInputs {
    fn default() -> Self {
        Self {
            kernel: None,
            claim: None,
            trust: TrustState::Untrusted,
            conformance: ConformanceState::Untested,
            policy: None,
        }
    }
}

/// Computes the kernel-effective contract from `inputs`.
///
/// The result is never safer than the least-safe contributing evidence on any
/// axis, and never stronger than `EffectContract::unknown()` when no source
/// contributes.
pub fn resolve(inputs: &ResolveInputs) -> EffectContract {
    let claim = match (inputs.trust, inputs.conformance) {
        (TrustState::Trusted, ConformanceState::Passed) => inputs.claim.as_ref(),
        _ => None,
    };
    let sources: Vec<&DeclaredSemantics> = [inputs.kernel.as_ref(), claim, inputs.policy.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    // A declared `Unspecified` always outranks real evidence, then normalizes
    // to the most conservative *persistable* variant — the schema CHECK
    // rejects wire value 0, so `Unspecified` itself can never be stored.
    EffectContract {
        effect_class: weakest(&sources, |s| s.effect_class, class_rank)
            .map(|class| match class {
                EffectClass::Unspecified => EffectClass::Opaque,
                other => other,
            })
            .unwrap_or(EffectClass::Opaque),
        idempotency: weakest(&sources, |s| s.idempotency, idempotency_rank)
            .map(|semantics| match semantics {
                IdempotencySemantics::Unspecified => IdempotencySemantics::UnknownIdempotency,
                other => other,
            })
            .unwrap_or(IdempotencySemantics::UnknownIdempotency),
        reconciliation: weakest(&sources, |s| s.reconciliation, reconciliation_rank)
            .map(|semantics| match semantics {
                ReconciliationSemantics::Unspecified => {
                    ReconciliationSemantics::UnknownReconciliation
                }
                other => other,
            })
            .unwrap_or(ReconciliationSemantics::UnknownReconciliation),
        cancellation: weakest(&sources, |s| s.cancellation, cancellation_rank)
            .unwrap_or(CancellationSemantics::Unsupported),
        compensation_capability: resolve_compensation(&sources),
    }
}

/// Kernel-known semantics for in-tree operations (`kernel` evidence source).
///
/// These are operations the kernel itself defines — the fixture adapter
/// operations shipped for conformance/integration testing — so their
/// semantics are authoritative evidence, not a claim. Unknown operations
/// return `DeclaredSemantics::default()` (no contributing evidence).
pub fn kernel_declared(operation: &str) -> DeclaredSemantics {
    match operation {
        "fixture.increment_counter" => DeclaredSemantics {
            effect_class: Some(EffectClass::ExternalMutation),
            idempotency: Some(IdempotencySemantics::IdempotencyKeySupported),
            reconciliation: Some(ReconciliationSemantics::StatusLookup),
            cancellation: Some(CancellationSemantics::BeforeDispatch),
            compensation_capability: None,
        },
        // Non-reconcilable mode for the INT-003 blocked-run path: the kernel
        // knows this operation cannot be observed after dispatch.
        "fixture.unreconcilable_counter" => DeclaredSemantics {
            effect_class: Some(EffectClass::ExternalMutation),
            idempotency: Some(IdempotencySemantics::NotIdempotent),
            reconciliation: Some(ReconciliationSemantics::Impossible),
            cancellation: Some(CancellationSemantics::Unsupported),
            compensation_capability: None,
        },
        _ => DeclaredSemantics::default(),
    }
}

/// Returns the least-safe contributed value for one axis.
fn weakest<T: Copy>(
    sources: &[&DeclaredSemantics],
    field: impl Fn(&DeclaredSemantics) -> Option<T>,
    rank: impl Fn(T) -> u8,
) -> Option<T> {
    sources
        .iter()
        .filter_map(|source| field(source))
        .max_by_key(|value| rank(*value))
}

/// Compensation is honoured only when a contributing source declares it and
/// every present policy also declares the same capability.
fn resolve_compensation(sources: &[&DeclaredSemantics]) -> Option<String> {
    let declared = sources
        .iter()
        .find_map(|s| s.compensation_capability.clone())?;
    let permitted = sources.iter().all(|s| {
        s.compensation_capability
            .as_deref()
            .is_none_or(|c| c == declared)
    });
    permitted.then_some(declared)
}

/// Danger ordering for `EffectClass`: higher is less safe.
const fn class_rank(class: EffectClass) -> u8 {
    match class {
        EffectClass::ReadOnly => 0,
        EffectClass::LocalMutation => 1,
        EffectClass::ExternalMutation => 2,
        EffectClass::Opaque => 3,
        EffectClass::Unspecified => 4,
    }
}

/// Safety ordering for `IdempotencySemantics`: higher is less safe.
const fn idempotency_rank(semantics: IdempotencySemantics) -> u8 {
    match semantics {
        IdempotencySemantics::NaturallyIdempotent => 0,
        IdempotencySemantics::IdempotencyKeySupported => 1,
        IdempotencySemantics::NotIdempotent => 2,
        IdempotencySemantics::UnknownIdempotency => 3,
        IdempotencySemantics::Unspecified => 4,
    }
}

/// Capability ordering for `ReconciliationSemantics`: higher is less capable.
const fn reconciliation_rank(semantics: ReconciliationSemantics) -> u8 {
    match semantics {
        ReconciliationSemantics::ResultLookup => 0,
        ReconciliationSemantics::DeterministicInspection => 1,
        ReconciliationSemantics::StatusLookup => 2,
        ReconciliationSemantics::Impossible => 3,
        ReconciliationSemantics::UnknownReconciliation => 4,
        ReconciliationSemantics::Unspecified => 5,
    }
}

/// Capability ordering for `CancellationSemantics`: higher is less capable.
const fn cancellation_rank(semantics: CancellationSemantics) -> u8 {
    match semantics {
        CancellationSemantics::BeforeDispatch => 0,
        CancellationSemantics::Cooperative => 1,
        CancellationSemantics::ProviderSpecific => 2,
        CancellationSemantics::Unsupported => 3,
    }
}

#[cfg(test)]
mod tests {
    use domain::effect::{EffectClass, IdempotencySemantics, ReconciliationSemantics};
    use domain::security::{ConformanceState, TrustState};

    use super::{DeclaredSemantics, ResolveInputs, resolve};
    use crate::contract::{CancellationSemantics, EffectContract};

    fn declared(
        class: EffectClass,
        idempotency: IdempotencySemantics,
        reconciliation: ReconciliationSemantics,
    ) -> DeclaredSemantics {
        DeclaredSemantics {
            effect_class: Some(class),
            idempotency: Some(idempotency),
            reconciliation: Some(reconciliation),
            cancellation: Some(CancellationSemantics::Cooperative),
            compensation_capability: None,
        }
    }

    #[test]
    fn untrusted_self_declared_read_only_cannot_self_promote() {
        let inputs = ResolveInputs {
            claim: Some(declared(
                EffectClass::ReadOnly,
                IdempotencySemantics::NaturallyIdempotent,
                ReconciliationSemantics::ResultLookup,
            )),
            trust: TrustState::Untrusted,
            conformance: ConformanceState::Passed,
            ..ResolveInputs::default()
        };
        let resolved = resolve(&inputs);
        assert_eq!(resolved.effect_class, EffectClass::Opaque);
        assert_eq!(
            resolved.idempotency,
            IdempotencySemantics::UnknownIdempotency
        );
        assert_eq!(
            resolved.reconciliation,
            ReconciliationSemantics::UnknownReconciliation
        );
    }

    #[test]
    fn untested_trusted_claim_does_not_contribute() {
        let inputs = ResolveInputs {
            claim: Some(declared(
                EffectClass::ReadOnly,
                IdempotencySemantics::NaturallyIdempotent,
                ReconciliationSemantics::ResultLookup,
            )),
            trust: TrustState::Trusted,
            conformance: ConformanceState::Untested,
            ..ResolveInputs::default()
        };
        assert_eq!(resolve(&inputs), EffectContract::unknown());
    }

    #[test]
    fn trusted_builtin_known_read_only_remains_read_only() {
        let inputs = ResolveInputs {
            kernel: Some(declared(
                EffectClass::ReadOnly,
                IdempotencySemantics::NaturallyIdempotent,
                ReconciliationSemantics::ResultLookup,
            )),
            ..ResolveInputs::default()
        };
        let resolved = resolve(&inputs);
        assert_eq!(resolved.effect_class, EffectClass::ReadOnly);
        assert_eq!(
            resolved.idempotency,
            IdempotencySemantics::NaturallyIdempotent
        );
        assert_eq!(
            resolved.reconciliation,
            ReconciliationSemantics::ResultLookup
        );
    }

    #[test]
    fn truth_table_intersection_never_promotes() {
        let safe = declared(
            EffectClass::ReadOnly,
            IdempotencySemantics::NaturallyIdempotent,
            ReconciliationSemantics::ResultLookup,
        );
        let unsafe_claim = declared(
            EffectClass::ExternalMutation,
            IdempotencySemantics::NotIdempotent,
            ReconciliationSemantics::Impossible,
        );
        let inputs = ResolveInputs {
            kernel: Some(safe),
            claim: Some(unsafe_claim),
            trust: TrustState::Trusted,
            conformance: ConformanceState::Passed,
            policy: None,
        };
        let resolved = resolve(&inputs);
        assert_eq!(resolved.effect_class, EffectClass::ExternalMutation);
        assert_eq!(resolved.idempotency, IdempotencySemantics::NotIdempotent);
        assert_eq!(resolved.reconciliation, ReconciliationSemantics::Impossible);
    }

    #[test]
    fn policy_contributes_even_without_a_claim() {
        let inputs = ResolveInputs {
            policy: Some(declared(
                EffectClass::LocalMutation,
                IdempotencySemantics::IdempotencyKeySupported,
                ReconciliationSemantics::StatusLookup,
            )),
            ..ResolveInputs::default()
        };
        let resolved = resolve(&inputs);
        assert_eq!(resolved.effect_class, EffectClass::LocalMutation);
        assert_eq!(
            resolved.idempotency,
            IdempotencySemantics::IdempotencyKeySupported
        );
        assert_eq!(
            resolved.reconciliation,
            ReconciliationSemantics::StatusLookup
        );
    }

    #[test]
    fn failed_conformance_blocks_claims() {
        let inputs = ResolveInputs {
            claim: Some(declared(
                EffectClass::ReadOnly,
                IdempotencySemantics::NaturallyIdempotent,
                ReconciliationSemantics::ResultLookup,
            )),
            trust: TrustState::Trusted,
            conformance: ConformanceState::Failed,
            ..ResolveInputs::default()
        };
        assert_eq!(resolve(&inputs), EffectContract::unknown());
    }

    #[test]
    fn compensation_requires_unanimous_sources() {
        let mut with_comp = declared(
            EffectClass::ExternalMutation,
            IdempotencySemantics::NotIdempotent,
            ReconciliationSemantics::Impossible,
        );
        with_comp.compensation_capability = Some("compensate".to_owned());
        let resolved = resolve(&ResolveInputs {
            kernel: Some(with_comp.clone()),
            ..ResolveInputs::default()
        });
        assert_eq!(
            resolved.compensation_capability.as_deref(),
            Some("compensate")
        );
        // A policy silent on compensation does not veto the kernel evidence.
        let resolved = resolve(&ResolveInputs {
            kernel: Some(with_comp.clone()),
            policy: Some(DeclaredSemantics::default()),
            ..ResolveInputs::default()
        });
        assert_eq!(
            resolved.compensation_capability.as_deref(),
            Some("compensate")
        );
        // A policy declaring a different capability conflicts: resolve to none.
        let conflicting = DeclaredSemantics {
            compensation_capability: Some("other".to_owned()),
            ..DeclaredSemantics::default()
        };
        let resolved = resolve(&ResolveInputs {
            kernel: Some(with_comp),
            policy: Some(conflicting),
            ..ResolveInputs::default()
        });
        assert_eq!(resolved.compensation_capability, None);
    }
}
