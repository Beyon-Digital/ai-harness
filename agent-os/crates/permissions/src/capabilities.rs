//! Capability catalogue from `contracts/capabilities/security-capabilities.yaml`.
//!
//! The catalogue is the single source of truth the engine consults: a
//! capability exists only when both its family token and its action token are
//! declared for that family. Parsing rejects unknown families, unknown
//! actions, missing separators, and pairs the contract does not define.

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily};

/// One family entry of the capability contract.
struct FamilySpec {
    token: &'static str,
    family: CapabilityFamily,
    actions: &'static [CapabilityAction],
}

const FAMILY_SPECS: [FamilySpec; 8] = [
    FamilySpec {
        token: "workspace",
        family: CapabilityFamily::Workspace,
        actions: &[
            CapabilityAction::Read,
            CapabilityAction::Write,
            CapabilityAction::Fork,
            CapabilityAction::Merge,
            CapabilityAction::TransferExclusive,
            CapabilityAction::SharedCoordinated,
        ],
    },
    FamilySpec {
        token: "network",
        family: CapabilityFamily::Network,
        actions: &[CapabilityAction::Connect],
    },
    FamilySpec {
        token: "secret",
        family: CapabilityFamily::Secret,
        actions: &[CapabilityAction::Use, CapabilityAction::SignOrAct],
    },
    FamilySpec {
        token: "agent",
        family: CapabilityFamily::Agent,
        actions: &[CapabilityAction::Spawn],
    },
    FamilySpec {
        token: "extension",
        family: CapabilityFamily::Extension,
        actions: &[
            CapabilityAction::Install,
            CapabilityAction::Enable,
            CapabilityAction::Disable,
        ],
    },
    FamilySpec {
        token: "config",
        family: CapabilityFamily::Config,
        actions: &[
            CapabilityAction::Propose,
            CapabilityAction::Test,
            CapabilityAction::Activate,
            CapabilityAction::Rollback,
        ],
    },
    FamilySpec {
        token: "effect",
        family: CapabilityFamily::Effect,
        actions: &[
            CapabilityAction::Reconcile,
            CapabilityAction::ResolveUnknown,
        ],
    },
    FamilySpec {
        token: "resource",
        family: CapabilityFamily::Resource,
        actions: &[CapabilityAction::Reserve],
    },
];

/// Returns the actions the contract declares for `family`.
pub fn actions(family: CapabilityFamily) -> &'static [CapabilityAction] {
    FAMILY_SPECS
        .iter()
        .find(|spec| spec.family == family)
        .map_or(&[], |spec| spec.actions)
}

/// Returns whether `capability` is a declared family/action pair.
pub fn supports(capability: &Capability) -> bool {
    actions(capability.family).contains(&capability.action)
}

/// Parses a canonical `family.action` token strictly against the contract.
///
/// Unknown families, unknown actions, missing separators, and unsupported
/// pairs are rejected as `InvalidArgument`/`Never`.
pub fn parse_capability(token: &str) -> errors::Result<Capability> {
    let (family_token, action_token) = token
        .split_once('.')
        .ok_or_else(|| unsupported("capability token has no family separator"))?;
    let spec = FAMILY_SPECS
        .iter()
        .find(|spec| spec.token == family_token)
        .ok_or_else(|| unsupported("capability family is not in the contract"))?;
    let action = spec
        .actions
        .iter()
        .find(|action| action.as_str() == action_token)
        .copied()
        .ok_or_else(|| unsupported("capability action is not in the contract for its family"))?;
    Ok(Capability::new(spec.family, action))
}

/// Builds an off-contract failure.
fn unsupported(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}
