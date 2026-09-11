//! Property tests: mirror enums either round-trip the exact wire value or fail
//! without fabricating a variant (R15.1, R15.2, P1).

use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
use domain::resource::{
    DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{
    ApprovalState, ConformanceState, RetentionClass, SensitivityClass, TrustState,
};
use proptest::prelude::*;

mod prop_enums {
    use super::*;

    macro_rules! wire_round_trip_prop {
        ($fn_name:ident, $mirror:ty) => {
            proptest! {
                #[test]
                fn $fn_name(value in any::<i32>()) {
                    match <$mirror>::from_wire(value) {
                        Ok(variant) => prop_assert_eq!(variant.to_wire(), value),
                        Err(error) => prop_assert_eq!(error.value, value),
                    }
                }
            }
        };
    }

    wire_round_trip_prop!(run_state, RunState);
    wire_round_trip_prop!(recovery_disposition, RecoveryDisposition);
    wire_round_trip_prop!(effect_class, EffectClass);
    wire_round_trip_prop!(effect_state, EffectState);
    wire_round_trip_prop!(idempotency_semantics, IdempotencySemantics);
    wire_round_trip_prop!(reconciliation_semantics, ReconciliationSemantics);
    wire_round_trip_prop!(sensitivity_class, SensitivityClass);
    wire_round_trip_prop!(retention_class, RetentionClass);
    wire_round_trip_prop!(trust_state, TrustState);
    wire_round_trip_prop!(conformance_state, ConformanceState);
    wire_round_trip_prop!(approval_state, ApprovalState);
    wire_round_trip_prop!(workspace_access_mode, WorkspaceAccessMode);
    wire_round_trip_prop!(lease_enforcement_state, LeaseEnforcementState);
    wire_round_trip_prop!(timer_state, TimerState);
    wire_round_trip_prop!(reservation_state, ReservationState);
    wire_round_trip_prop!(dependency_condition, DependencyCondition);

    #[test]
    fn unknown_wire_values_are_rejected_with_their_context() {
        let error = RunState::from_wire(99).unwrap_err();
        assert_eq!(error.value, 99);
        assert_eq!(error.enum_name, "RunState");

        let error = EffectClass::from_wire(-1).unwrap_err();
        assert_eq!(error.value, -1);
        assert_eq!(error.enum_name, "EffectClass");

        let error = i32::MAX;
        assert_eq!(ReservationState::from_wire(error).unwrap_err().value, error);
    }

    #[test]
    fn wire_numbers_follow_the_contract_snapshot() {
        assert_eq!(RunState::Unspecified.to_wire(), 0);
        assert_eq!(RunState::Created.to_wire(), 1);
        assert_eq!(RunState::Ready.to_wire(), 2);
        assert_eq!(RunState::Running.to_wire(), 3);
        assert_eq!(RunState::WaitingTool.to_wire(), 4);
        assert_eq!(RunState::WaitingChild.to_wire(), 5);
        assert_eq!(RunState::WaitingHuman.to_wire(), 6);
        assert_eq!(RunState::Suspended.to_wire(), 7);
        assert_eq!(RunState::Cancelling.to_wire(), 8);
        assert_eq!(RunState::Completed.to_wire(), 9);
        assert_eq!(RunState::Failed.to_wire(), 10);
        assert_eq!(RunState::Cancelled.to_wire(), 11);

        assert_eq!(RecoveryDisposition::Unspecified.to_wire(), 0);
        assert_eq!(RecoveryDisposition::Normal.to_wire(), 1);
        assert_eq!(RecoveryDisposition::NeedsReconciliation.to_wire(), 2);
        assert_eq!(RecoveryDisposition::Recovering.to_wire(), 3);
        assert_eq!(RecoveryDisposition::BlockedUnknownEffect.to_wire(), 4);
        assert_eq!(RecoveryDisposition::BlockedMissingResource.to_wire(), 5);
        assert_eq!(RecoveryDisposition::RequiresHumanDecision.to_wire(), 6);

        assert_eq!(EffectClass::Unspecified.to_wire(), 0);
        assert_eq!(EffectClass::ReadOnly.to_wire(), 1);
        assert_eq!(EffectClass::LocalMutation.to_wire(), 2);
        assert_eq!(EffectClass::ExternalMutation.to_wire(), 3);
        assert_eq!(EffectClass::Opaque.to_wire(), 4);

        assert_eq!(EffectState::Unspecified.to_wire(), 0);
        assert_eq!(EffectState::Prepared.to_wire(), 1);
        assert_eq!(EffectState::Claimed.to_wire(), 2);
        assert_eq!(EffectState::Dispatched.to_wire(), 3);
        assert_eq!(EffectState::Acknowledged.to_wire(), 4);
        assert_eq!(EffectState::Committed.to_wire(), 5);
        assert_eq!(EffectState::Failed.to_wire(), 6);
        assert_eq!(EffectState::Cancelled.to_wire(), 7);
        assert_eq!(EffectState::Unknown.to_wire(), 8);

        assert_eq!(IdempotencySemantics::Unspecified.to_wire(), 0);
        assert_eq!(IdempotencySemantics::NaturallyIdempotent.to_wire(), 1);
        assert_eq!(IdempotencySemantics::IdempotencyKeySupported.to_wire(), 2);
        assert_eq!(IdempotencySemantics::NotIdempotent.to_wire(), 3);
        assert_eq!(IdempotencySemantics::UnknownIdempotency.to_wire(), 4);

        assert_eq!(ReconciliationSemantics::Unspecified.to_wire(), 0);
        assert_eq!(ReconciliationSemantics::StatusLookup.to_wire(), 1);
        assert_eq!(ReconciliationSemantics::ResultLookup.to_wire(), 2);
        assert_eq!(
            ReconciliationSemantics::DeterministicInspection.to_wire(),
            3
        );
        assert_eq!(ReconciliationSemantics::Impossible.to_wire(), 4);
        assert_eq!(ReconciliationSemantics::UnknownReconciliation.to_wire(), 5);

        assert_eq!(SensitivityClass::Unspecified.to_wire(), 0);
        assert_eq!(SensitivityClass::Public.to_wire(), 1);
        assert_eq!(SensitivityClass::Internal.to_wire(), 2);
        assert_eq!(SensitivityClass::Private.to_wire(), 3);
        assert_eq!(SensitivityClass::Secret.to_wire(), 4);

        assert_eq!(RetentionClass::Unspecified.to_wire(), 0);
        assert_eq!(RetentionClass::Ephemeral.to_wire(), 1);
        assert_eq!(RetentionClass::Session.to_wire(), 2);
        assert_eq!(RetentionClass::Audit.to_wire(), 3);
        assert_eq!(RetentionClass::Durable.to_wire(), 4);

        assert_eq!(TrustState::Unspecified.to_wire(), 0);
        assert_eq!(TrustState::Trusted.to_wire(), 1);
        assert_eq!(TrustState::Untrusted.to_wire(), 2);

        assert_eq!(ConformanceState::Unspecified.to_wire(), 0);
        assert_eq!(ConformanceState::Untested.to_wire(), 1);
        assert_eq!(ConformanceState::Passed.to_wire(), 2);
        assert_eq!(ConformanceState::Failed.to_wire(), 3);

        assert_eq!(ApprovalState::Unspecified.to_wire(), 0);
        assert_eq!(ApprovalState::Pending.to_wire(), 1);
        assert_eq!(ApprovalState::Approved.to_wire(), 2);
        assert_eq!(ApprovalState::Denied.to_wire(), 3);
        assert_eq!(ApprovalState::Expired.to_wire(), 4);

        assert_eq!(WorkspaceAccessMode::Unspecified.to_wire(), 0);
        assert_eq!(WorkspaceAccessMode::ReadOnly.to_wire(), 1);
        assert_eq!(WorkspaceAccessMode::ExclusiveWrite.to_wire(), 2);
        assert_eq!(WorkspaceAccessMode::IsolatedFork.to_wire(), 3);
        assert_eq!(WorkspaceAccessMode::SharedCoordinatedWrite.to_wire(), 4);

        assert_eq!(LeaseEnforcementState::Unspecified.to_wire(), 0);
        assert_eq!(LeaseEnforcementState::Active.to_wire(), 1);
        assert_eq!(LeaseEnforcementState::Revoked.to_wire(), 2);

        assert_eq!(TimerState::Unspecified.to_wire(), 0);
        assert_eq!(TimerState::Scheduled.to_wire(), 1);
        assert_eq!(TimerState::Claimed.to_wire(), 2);
        assert_eq!(TimerState::Fired.to_wire(), 3);
        assert_eq!(TimerState::Cancelled.to_wire(), 4);

        assert_eq!(ReservationState::Unspecified.to_wire(), 0);
        assert_eq!(ReservationState::Reserved.to_wire(), 1);
        assert_eq!(ReservationState::Allocated.to_wire(), 2);
        assert_eq!(ReservationState::Released.to_wire(), 3);
        assert_eq!(ReservationState::Expired.to_wire(), 4);
        assert_eq!(ReservationState::Unknown.to_wire(), 5);

        assert_eq!(DependencyCondition::Unspecified.to_wire(), 0);
        assert_eq!(DependencyCondition::CompletedSuccessfully.to_wire(), 1);
        assert_eq!(DependencyCondition::AnyTerminal.to_wire(), 2);
        assert_eq!(DependencyCondition::CompletedOrCancelled.to_wire(), 3);
    }
}
