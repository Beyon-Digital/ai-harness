//! One deterministic decision over capabilities, scopes, chains, and joint risk.
//!
//! `evaluate` is pure: it performs no I/O, mutates nothing, and returns the
//! same `Decision` for identical inputs. Authority is computed as an
//! intersection, never inferred from an individual hop:
//!
//! - every hop's grant-backed capability set must contain the requested
//!   capability, so a child can never exercise authority an ancestor narrowed
//!   away (`AncestorConstraint`), and a structurally invalid chain fails
//!   closed as `NoGrant`;
//! - the tool's allow list, when present, is intersected on top of the chain
//!   (`ToolRestriction`), so a privileged helper cannot be confused into
//!   exceeding its caller's grants;
//! - the scoped target must match the capability's family and pass the scope
//!   rules below (`OutOfScope`).
//!
//! Scope rules:
//!
//! - workspace: the `uri` root is normalized (canonical scheme, no `.` or
//!   `..` segments); an optional `path` is joined to the root, or, when it is
//!   itself absolute (`scheme://`), prefix-matched against the root. Traversal
//!   and foreign roots are rejected. Path comparison is segment-aware;
//!   workspace paths are case-sensitive.
//! - network: every domain is lowercased and validated as a DNS name. `*`
//!   matches any host; a token that begins with `.` (for example
//!   `.example.com`) is the documented suffix rule and matches the named
//!   domain and all of its subdomains; every other token matches exactly.
//!   [`domain_matches`] applies the rule to a concrete host.
//! - secret: the `uri` is normalized like a workspace root and an optional
//!   `egress` list is validated with the network domain rule.
//! - config: the target operation must equal the requested action.
//! - agent: `ChildCount` must allow at least one child.
//! - extension, effect, and resource capabilities are unscoped and require
//!   [`ScopedTarget::None`].
//!
//! Joint risk: a secret `Use` or `SignOrAct` request whose effective authority
//! also holds `Network Connect`, or whose secret target declares an egress
//! list, is upgraded from `Allow` to `RequireApproval`. The draft cites both
//! capabilities and carries a deterministic nonce, so the upgrade is stable
//! for identical inputs; approval lifetimes default to
//! [`DEFAULT_APPROVAL_TTL_MS`] past `now_ms`. Denials never carry target or
//! secret content.

use std::collections::BTreeSet;

use domain::ids::{ActorId, CapabilityGrantId, PrincipalId, RunId};
use identity::delegation::{
    Capability, CapabilityAction, CapabilityFamily, DelegationChain, Hop, validate_chain,
};

use crate::capabilities;

/// Default approval lifetime added to `now_ms` when a draft is produced.
pub const DEFAULT_APPROVAL_TTL_MS: i64 = 15 * 60 * 1_000;

/// The target an operation is scoped to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopedTarget {
    /// A workspace root plus an optional path inside it.
    Workspace {
        /// Workspace root URI.
        uri: String,
        /// Optional path relative to the root, or an absolute in-root path.
        path: Option<String>,
    },
    /// Network destinations under the documented exact/suffix/wildcard rule.
    Network {
        /// Allowed destination domains.
        domains: Vec<String>,
    },
    /// A secret URI plus an optional egress list of destination domains.
    Secret {
        /// Secret resource URI.
        uri: String,
        /// Optional destinations the material may flow to.
        egress: Option<Vec<String>>,
    },
    /// A configuration operation.
    Config {
        /// Operation the configuration target applies to.
        operation: CapabilityAction,
    },
    /// A bound on the number of children an agent operation may create.
    ChildCount {
        /// Maximum number of children.
        max: u32,
    },
    /// No scoped target; valid only for unscoped families.
    None,
}

/// Why a request was denied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// No hop in the chain carries the requested capability.
    NoGrant,
    /// The target is malformed or outside the capability's scope.
    OutOfScope,
    /// An ancestor hop narrowed away the capability the tip claims.
    AncestorConstraint,
    /// The tool's allow list excludes the requested capability.
    ToolRestriction,
    /// A grant backing the request has lapsed.
    ExpiredGrant,
    /// The capability pair is not in the contract.
    Unsupported,
}

impl DenyReason {
    /// Returns the stable lowercase token for this reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoGrant => "no_grant",
            Self::OutOfScope => "out_of_scope",
            Self::AncestorConstraint => "ancestor_constraint",
            Self::ToolRestriction => "tool_restriction",
            Self::ExpiredGrant => "expired_grant",
            Self::Unsupported => "unsupported",
        }
    }
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The engine's single decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The request is authorized by the cited grants.
    Allow {
        /// Grant identifiers backing the decision.
        grant_refs: Vec<CapabilityGrantId>,
    },
    /// The request is refused; the reason never carries secret content.
    Deny {
        /// Stable denial reason.
        reason: DenyReason,
    },
    /// The request needs an operator approval bound to this draft.
    RequireApproval {
        /// Immutable draft a caller can turn into an approval request.
        request: ApprovalDraft,
    },
}

/// The content of a pending approval request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalDraft {
    /// Canonical `family.action` operation token.
    pub operation: String,
    /// Canonical target token; identifiers and destinations only.
    pub target: String,
    /// Exercised capabilities, sorted by family then action.
    pub capabilities: Vec<Capability>,
    /// Extension bundle digest when the operation is extension-bound.
    pub extension_digest: Option<String>,
    /// Configuration generation digest when the operation is config-bound.
    pub config_digest: Option<String>,
    /// Expiry of the requested approval.
    pub expiry_ms: i64,
    /// Deterministic request nonce.
    pub nonce: String,
}

/// Everything the engine needs to authorize one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    /// Principal on whose behalf the operation runs.
    pub principal_id: PrincipalId,
    /// Actor exercising the authority.
    pub actor_id: ActorId,
    /// Run context when the operation belongs to a run.
    pub run_id: Option<RunId>,
    /// Loaded delegation chain from root to tip.
    pub chain: DelegationChain,
    /// The tool's own allow list, when it has one.
    pub tool_capabilities: Option<Vec<Capability>>,
    /// Capability the operation requests.
    pub capability: Capability,
    /// Target the operation is scoped to.
    pub target: ScopedTarget,
    /// Evaluation time.
    pub now_ms: i64,
}

/// Evaluates a permission request, returning exactly one decision variant.
pub fn evaluate(request: &PermissionRequest) -> Decision {
    let capability = request.capability;
    if !capabilities::supports(&capability) {
        return Decision::Deny {
            reason: DenyReason::Unsupported,
        };
    }

    let ancestor_authority = ancestor_capabilities(&request.chain);
    if !ancestor_authority.contains(&capability) {
        let tip_holds = request
            .chain
            .hops
            .last()
            .is_some_and(|tip| tip.capabilities.contains(&capability));
        let reason = if tip_holds {
            DenyReason::AncestorConstraint
        } else {
            DenyReason::NoGrant
        };
        return Decision::Deny { reason };
    }
    if validate_chain(&request.chain).is_err() {
        return Decision::Deny {
            reason: DenyReason::NoGrant,
        };
    }

    let mut effective = ancestor_authority;
    if let Some(tool_capabilities) = &request.tool_capabilities {
        effective.retain(|held| tool_capabilities.contains(held));
        if !effective.contains(&capability) {
            return Decision::Deny {
                reason: DenyReason::ToolRestriction,
            };
        }
    }

    let target_token = match scoped_target_token(&capability, &request.target) {
        Ok(token) => token,
        Err(reason) => return Decision::Deny { reason },
    };

    if let Some(exercised) = joint_risk(&capability, &request.target, &effective) {
        return Decision::RequireApproval {
            request: approval_draft(request, target_token, &exercised),
        };
    }

    Decision::Allow {
        grant_refs: tip_grant_refs(request.chain.hops.last()),
    }
}

/// Applies the documented exact/suffix/wildcard rule to a concrete host.
///
/// The rule is normalized first: matching is case-insensitive, `*` matches any
/// host, a leading dot matches the named domain and every subdomain, and every
/// other token matches exactly.
pub fn domain_matches(rule: &str, host: &str) -> bool {
    let Some(rule) = normalize_domain(rule) else {
        return false;
    };
    let Some(host) = normalize_domain(host) else {
        return false;
    };
    if host == "*" {
        return false;
    }
    if rule == "*" {
        return true;
    }
    match rule.strip_prefix('.') {
        Some(suffix) => host == suffix || host.ends_with(&rule),
        None => host == rule,
    }
}

/// Intersects every hop's capability set, root through tip.
fn ancestor_capabilities(chain: &DelegationChain) -> BTreeSet<Capability> {
    let mut intersection: Option<BTreeSet<Capability>> = None;
    for hop in &chain.hops {
        let held: BTreeSet<Capability> = hop.capabilities.iter().copied().collect();
        intersection = Some(match intersection {
            Some(current) => current.intersection(&held).copied().collect(),
            None => held,
        });
    }
    intersection.unwrap_or_default()
}

/// Returns the tip hop's grant identifiers, sorted and duplicate-free.
fn tip_grant_refs(tip: Option<&Hop>) -> Vec<CapabilityGrantId> {
    let mut refs = BTreeSet::new();
    if let Some(tip) = tip {
        refs.extend(tip.grant_ids.iter().copied());
    }
    refs.into_iter().collect()
}

/// Upgrades a secret request to approval when joint risk is present.
fn joint_risk(
    capability: &Capability,
    target: &ScopedTarget,
    effective: &BTreeSet<Capability>,
) -> Option<BTreeSet<Capability>> {
    if capability.family != CapabilityFamily::Secret
        || !matches!(
            capability.action,
            CapabilityAction::Use | CapabilityAction::SignOrAct
        )
    {
        return None;
    }
    let connect = Capability::new(CapabilityFamily::Network, CapabilityAction::Connect);
    let connect_held = effective.contains(&connect);
    let egress_declared = matches!(
        target,
        ScopedTarget::Secret {
            egress: Some(_),
            ..
        }
    );
    if !connect_held && !egress_declared {
        return None;
    }
    let mut exercised = BTreeSet::new();
    exercised.insert(*capability);
    if connect_held {
        exercised.insert(connect);
    }
    Some(exercised)
}

/// Builds the deterministic draft for an upgraded request.
fn approval_draft(
    request: &PermissionRequest,
    target_token: String,
    exercised: &BTreeSet<Capability>,
) -> ApprovalDraft {
    let nonce = deterministic_nonce(request, &target_token);
    ApprovalDraft {
        operation: request.capability.to_string(),
        target: target_token,
        capabilities: exercised.iter().copied().collect(),
        extension_digest: None,
        config_digest: None,
        expiry_ms: request.now_ms.saturating_add(DEFAULT_APPROVAL_TTL_MS),
        nonce,
    }
}

/// Derives a stable nonce from the request identity and target.
///
/// The engine is pure, so the nonce cannot be random; it is a canonical
/// fingerprint of the inputs and the approval digest binds it.
fn deterministic_nonce(request: &PermissionRequest, target_token: &str) -> String {
    let run = request
        .run_id
        .map_or_else(|| "none".to_string(), |run| run.to_string());
    format!(
        "v1|{}|{}|{}|{}|{}|{}",
        request.principal_id,
        request.actor_id,
        run,
        request.capability,
        target_token,
        request.now_ms
    )
}

/// Validates the target against the capability's family and renders its token.
fn scoped_target_token(
    capability: &Capability,
    target: &ScopedTarget,
) -> Result<String, DenyReason> {
    match (capability.family, target) {
        (CapabilityFamily::Workspace, ScopedTarget::Workspace { uri, path }) => {
            normalized_workspace(uri, path.as_deref()).ok_or(DenyReason::OutOfScope)
        }
        (CapabilityFamily::Network, ScopedTarget::Network { domains }) => {
            normalized_domains(domains).ok_or(DenyReason::OutOfScope)
        }
        (CapabilityFamily::Secret, ScopedTarget::Secret { uri, egress }) => {
            let uri = normalize_uri(uri).ok_or(DenyReason::OutOfScope)?;
            match egress {
                None => Ok(uri),
                Some(egress) => {
                    let egress = normalized_domains(egress).ok_or(DenyReason::OutOfScope)?;
                    Ok(format!("{uri}|egress={egress}"))
                }
            }
        }
        (CapabilityFamily::Agent, ScopedTarget::ChildCount { max }) if *max >= 1 => {
            Ok(format!("child_count:{max}"))
        }
        (CapabilityFamily::Config, ScopedTarget::Config { operation })
            if *operation == capability.action =>
        {
            Ok(operation.as_str().to_string())
        }
        (
            CapabilityFamily::Extension | CapabilityFamily::Effect | CapabilityFamily::Resource,
            ScopedTarget::None,
        ) => Ok("none".to_string()),
        _ => Err(DenyReason::OutOfScope),
    }
}

/// Normalizes a workspace root and confines a path to it.
fn normalized_workspace(uri: &str, path: Option<&str>) -> Option<String> {
    let root = normalize_uri(uri)?;
    let Some(path) = path else {
        return Some(root);
    };
    let candidate = if path.contains("://") {
        normalize_uri(path)?
    } else {
        format!("{root}/{}", normalize_relative_path(path)?)
    };
    if candidate == root || candidate.starts_with(&format!("{root}/")) {
        Some(candidate)
    } else {
        None
    }
}

/// Normalizes a `scheme://host/segments` URI, rejecting traversal.
fn normalize_uri(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    for segment in rest.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        if segment
            .chars()
            .any(|c| c.is_whitespace() || c == '\\' || c.is_control())
        {
            return None;
        }
    }
    Some(format!("{scheme}://{rest}"))
}

/// Normalizes a relative slash-separated path, rejecting traversal.
fn normalize_relative_path(path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') || path.contains("://") {
        return None;
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        if segment
            .chars()
            .any(|c| c.is_whitespace() || c == '\\' || c.is_control())
        {
            return None;
        }
    }
    Some(path.to_string())
}

/// Validates and canonicalizes a non-empty list of destination domains.
fn normalized_domains(domains: &[String]) -> Option<String> {
    if domains.is_empty() {
        return None;
    }
    let mut normalized = Vec::with_capacity(domains.len());
    for domain in domains {
        normalized.push(normalize_domain(domain)?);
    }
    normalized.sort();
    normalized.dedup();
    Some(normalized.join(","))
}

/// Lowercases and validates one domain token under the scope rule.
fn normalize_domain(token: &str) -> Option<String> {
    let lowered = token.to_ascii_lowercase();
    if lowered == "*" {
        return Some(lowered);
    }
    let (suffix, body) = match lowered.strip_prefix('.') {
        Some(body) => (true, body),
        None => (false, lowered.as_str()),
    };
    if body.is_empty() || body.len() > 253 {
        return None;
    }
    for label in body.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        let bytes = label.as_bytes();
        if !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return None;
        }
        if !bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return None;
        }
        if !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return None;
        }
    }
    Some(if suffix {
        format!(".{body}")
    } else {
        body.to_string()
    })
}
