//! Deterministic port resolver (`specs/adapter-registry.md` "Resolution").
//!
//! Given a `PortRequirement` and the registered adapters, pick one
//! candidate by: exact port major version → mandatory capabilities →
//! trust/conformance/sandbox policy → optional config pin → configured
//! priority → immutable identity tie-break. The resolved triple is
//! persisted into `ResolvedRunEnvironment`; the runtime never branches on
//! adapter/vendor names.
#![forbid(unsafe_code)]

use domain::ids::AdapterId;
use domain::security::{ConformanceState, TrustState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::AdapterRegistrationRow;

use crate::capabilities::{CapabilitySet, SandboxTier};
use crate::manifest::PortImpl;

/// What a run needs resolved.
#[derive(Clone, Debug)]
pub struct PortRequirement {
    /// Canonical port id.
    pub port_id: String,
    /// Required major version — exact match.
    pub port_version: u32,
    /// Mandatory adapter capabilities.
    pub required_capabilities: Vec<String>,
    /// Minimum sandbox tier the adapter must host.
    pub sandbox_tier: SandboxTier,
    /// Optional config/profile pin on adapter id.
    pub pin_adapter_id: Option<AdapterId>,
    /// When true, only adapters whose registration shows a `passed`
    /// conformance report are eligible (the required-review gate).
    pub require_conformance_passed: bool,
}

/// The resolver's answer — the immutable identity plus the negotiated
/// capability document to persist into the run environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAdapter {
    /// Registered adapter id.
    pub adapter_id: AdapterId,
    /// Registered version.
    pub version: String,
    /// Registered bundle digest.
    pub bundle_digest: String,
    /// Negotiated capability document: adapter's declared capabilities
    /// that satisfy the requirement (deterministic, sorted).
    pub negotiated_capabilities: Vec<String>,
    /// Implemented port entry that matched.
    pub port: PortImpl,
    /// Trust state of the selected registration.
    pub trust_state: TrustState,
}

/// Decoded candidate for resolution.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Registration row as persisted.
    pub row: AdapterRegistrationRow,
    /// Decoded `implemented_ports`.
    pub ports: Vec<PortImpl>,
    /// Decoded capability set.
    pub capabilities: CapabilitySet,
}

impl Candidate {
    /// Decodes the JSON blobs on a registration row.
    pub fn decode(row: AdapterRegistrationRow) -> errors::Result<Self> {
        let ports: Vec<PortImpl> = serde_json::from_slice(&row.implemented_ports).map_err(|e| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "persisted implemented_ports is not valid JSON",
            )
            .with_source(e)
        })?;
        let caps: Vec<String> = serde_json::from_slice(&row.capabilities).map_err(|e| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "persisted capabilities is not valid JSON",
            )
            .with_source(e)
        })?;
        Ok(Self {
            row,
            ports,
            capabilities: CapabilitySet::new(caps),
        })
    }
}

/// Resolves `requirement` against `candidates`.
///
/// `priority` lists preferred adapter ids in order; unlisted candidates
/// sort after listed ones. Remaining ties break on the immutable identity
/// `(adapter_id, version, bundle_digest)` — fully deterministic.
pub fn resolve(
    requirement: &PortRequirement,
    candidates: &[Candidate],
    priority: &[AdapterId],
) -> errors::Result<ResolvedAdapter> {
    let mut matches: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| {
            // 1. exact compatible major version
            c.ports.iter().any(|p| {
                p.port_id == requirement.port_id && p.port_version == requirement.port_version
            })
                // 2. mandatory capabilities
                && c.capabilities.satisfies(&requirement.required_capabilities)
                // 3. trust/sandbox policy: T2 requests fail closed unless
                // the adapter declares every required capability; an
                // untrusted registration can only serve T0/T1.
                && c.capabilities.hosts_tier(requirement.sandbox_tier)
                && (requirement.sandbox_tier <= SandboxTier::T0
                    || c.row.trust_state == TrustState::Trusted)
                // 4. config/profile pin
                && requirement
                    .pin_adapter_id
                    .is_none_or(|pin| pin == c.row.adapter_id)
                // conformance: a failed adapter is never selected; the
                // required-review gate additionally demands `passed`
                && c.row.conformance_state != ConformanceState::Failed
                && (!requirement.require_conformance_passed
                    || c.row.conformance_state == ConformanceState::Passed)
        })
        .collect();
    // 5. deterministic ordering: configured priority, then identity.
    matches.sort_by(|a, b| {
        let rank = |c: &Candidate| {
            priority
                .iter()
                .position(|p| *p == c.row.adapter_id)
                .unwrap_or(usize::MAX)
        };
        rank(a).cmp(&rank(b)).then_with(|| {
            (
                a.row.adapter_id.to_string(),
                a.row.version.clone(),
                a.row.bundle_digest.clone(),
            )
                .cmp(&(
                    b.row.adapter_id.to_string(),
                    b.row.version.clone(),
                    b.row.bundle_digest.clone(),
                ))
        })
    });
    let chosen = matches.first().ok_or_else(|| {
        KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!(
                "no adapter satisfies port {}@{} (tier {:?}): capability unsupported",
                requirement.port_id, requirement.port_version, requirement.sandbox_tier
            ),
        )
    })?;
    let port = chosen
        .ports
        .iter()
        .find(|p| p.port_id == requirement.port_id && p.port_version == requirement.port_version)
        .cloned()
        .expect("filtered on this port");
    Ok(ResolvedAdapter {
        adapter_id: chosen.row.adapter_id,
        version: chosen.row.version.clone(),
        bundle_digest: chosen.row.bundle_digest.clone(),
        negotiated_capabilities: chosen.capabilities.names(),
        port,
        trust_state: chosen.row.trust_state,
    })
}
