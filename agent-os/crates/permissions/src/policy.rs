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
//! Grant scopes (R2.3, D9): callers load [`GrantScope`] rows from
//! `capability_grants` for the grants their chain cites. `Allow` requires at
//! least one grant that the chain's tip holds whose capability matches the
//! request, that has not expired, and that either is unscoped (`scope: None`,
//! which allows any well-formed target) or whose scope covers the request
//! target. `Allow { grant_refs }` cites exactly the covering grants. Matching
//! grants that are all unexpired but none covering are `OutOfScope`; when
//! every matching grant has lapsed the reason is `ExpiredGrant`; when the
//! caller supplies no matching grant row at all the request fails closed as
//! `NoGrant`.
//!
//! Scope rules:
//!
//! - workspace: the `uri` root is normalized (canonical scheme, no `.` or
//!   `..` segments); an optional `path` is joined to the root, or, when it is
//!   itself absolute (`scheme://`), prefix-matched against the root. A grant
//!   scope covers a target when the normalized scopes are equal or the grant
//!   scope is a segment prefix of the target path. Traversal and foreign roots
//!   are rejected. Path comparison is segment-aware; workspace paths are
//!   case-sensitive.
//! - network: every domain is lowercased and validated as a DNS name. `*`
//!   matches any host; a token that begins with `.` (for example
//!   `.example.com`) is the documented suffix rule and matches the named
//!   domain and all of its subdomains; every other token matches exactly.
//!   [`domain_matches`] applies the rule to a concrete host. A network grant
//!   scope covers a request only when every requested domain is covered by
//!   some allowed rule.
//! - secret: the `uri` is normalized like a workspace root, and coverage is
//!   the same segment-prefix rule (a grant for `secret://prod` covers
//!   `secret://prod/api-key`). The optional `egress` list is validated with
//!   the network domain rule: a grant without egress only covers requests
//!   without egress, a bounded grant only covers bounded requests whose
//!   domains it matches, and an unbounded grant (`Some([])` or a `*` entry)
//!   covers any request egress.
//! - config: the grant scope's operation must equal the requested action.
//! - agent: the requested `ChildCount` must not exceed the grant scope's max.
//! - extension, effect, and resource capabilities are unscoped and require
//!   [`ScopedTarget::None`].
//!
//! Joint risk: a secret `Use` or `SignOrAct` request whose effective authority
//! also holds `Network Connect`, or whose secret target declares an unbounded
//! egress, is upgraded from `Allow` to `RequireApproval`. Egress is only
//! unbounded when the request's egress list contains the global `*` wildcard
//! or is empty (egress declared without bounding it); a bounded list of
//! concrete hosts or documented suffixes does not upgrade. The draft cites
//! both capabilities and carries a deterministic template nonce, so the
//! upgrade is stable for identical inputs; `approvals::create_request` assigns
//! the canonical nonce it binds (D10) and approval lifetimes default to
//! [`DEFAULT_APPROVAL_TTL_MS`] past `now_ms`. Denials never carry target or
//! secret content.

use std::collections::{BTreeMap, BTreeSet};

use domain::ids::{ActorId, CapabilityGrantId, PrincipalId, RunId};
use identity::delegation::{
    Capability, CapabilityAction, CapabilityFamily, DelegationChain, validate_chain,
};

use crate::capabilities;
use crate::{GrantScope, ScopedTarget};

/// Default approval lifetime added to `now_ms` when a draft is produced.
pub const DEFAULT_APPROVAL_TTL_MS: i64 = 15 * 60 * 1_000;

/// Why a request was denied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// No hop in the chain carries the requested capability, or no supplied
    /// grant row backs it.
    NoGrant,
    /// The target is malformed or outside every matching grant's scope.
    OutOfScope,
    /// An ancestor hop narrowed away the capability the tip claims.
    AncestorConstraint,
    /// The tool's allow list excludes the requested capability.
    ToolRestriction,
    /// Every matching grant has lapsed.
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
    /// Deterministic template nonce; `create_request` assigns the bound nonce.
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
    /// Grant rows loaded from `capability_grants` for the chain's grants.
    pub grant_scopes: Vec<GrantScope>,
    /// The tool's own allow list, when it has one.
    pub tool_capabilities: Option<Vec<Capability>>,
    /// Capability the operation requests.
    pub capability: Capability,
    /// Target the operation is scoped to.
    pub target: ScopedTarget,
    /// Extension bundle digest the approval should bind, when applicable.
    pub extension_digest: Option<String>,
    /// Configuration generation digest the approval should bind, when applicable.
    pub config_digest: Option<String>,
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

    let (backing, target_token) = match backing_grants(request, &capability) {
        Ok(resolved) => resolved,
        Err(reason) => return Decision::Deny { reason },
    };

    if let Some(exercised) = joint_risk(&capability, &request.target, &effective) {
        return Decision::RequireApproval {
            request: approval_draft(request, target_token, &exercised),
        };
    }

    Decision::Allow {
        grant_refs: backing,
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

/// Resolves the tip grants that back the request and the canonical target.
///
/// Only grants the chain's tip actually holds are considered; a matching grant
/// must be unexpired and either unscoped or covering the target. The returned
/// refs are exactly the covering grants.
fn backing_grants(
    request: &PermissionRequest,
    capability: &Capability,
) -> Result<(Vec<CapabilityGrantId>, String), DenyReason> {
    let tip_grants: BTreeSet<CapabilityGrantId> = request
        .chain
        .hops
        .last()
        .map(|tip| tip.grant_ids.iter().copied().collect())
        .unwrap_or_default();
    let mut rows: BTreeMap<CapabilityGrantId, Vec<&GrantScope>> = BTreeMap::new();
    for row in &request.grant_scopes {
        if tip_grants.contains(&row.grant_id) && row.capability == *capability {
            rows.entry(row.grant_id).or_default().push(row);
        }
    }
    if rows.is_empty() {
        return Err(DenyReason::NoGrant);
    }

    let target_token = scoped_target_token(capability, &request.target)?;
    let mut any_unexpired = false;
    let mut covering = BTreeSet::new();
    for (grant_id, rows) in &rows {
        let unexpired = rows
            .iter()
            .all(|row| !grant_expired(row.expires_at_ms, request.now_ms));
        let covers = rows.iter().all(|row| {
            row.scope
                .as_ref()
                .is_none_or(|scope| target_covers(scope, &request.target))
        });
        if unexpired {
            any_unexpired = true;
        }
        if unexpired && covers {
            covering.insert(*grant_id);
        }
    }

    if !covering.is_empty() {
        return Ok((covering.into_iter().collect(), target_token));
    }
    Err(if any_unexpired {
        DenyReason::OutOfScope
    } else {
        DenyReason::ExpiredGrant
    })
}

/// Returns whether a grant with this expiry has lapsed at `now_ms`.
fn grant_expired(expires_at_ms: Option<i64>, now_ms: i64) -> bool {
    expires_at_ms.is_some_and(|expiry| expiry <= now_ms)
}

/// Returns whether a grant scope covers a request target.
fn target_covers(scope: &ScopedTarget, target: &ScopedTarget) -> bool {
    match (scope, target) {
        (
            ScopedTarget::Workspace {
                uri: scope_uri,
                path: scope_path,
            },
            ScopedTarget::Workspace {
                uri: target_uri,
                path: target_path,
            },
        ) => match (
            normalized_workspace(scope_uri, scope_path.as_deref()),
            normalized_workspace(target_uri, target_path.as_deref()),
        ) {
            (Some(scope), Some(target)) => segment_prefix(&scope, &target),
            _ => false,
        },
        (
            ScopedTarget::Network { domains: allowed },
            ScopedTarget::Network { domains: requested },
        ) => requested
            .iter()
            .all(|domain| requested_domain_covered(allowed, domain)),
        (
            ScopedTarget::Secret {
                uri: scope_uri,
                egress: scope_egress,
            },
            ScopedTarget::Secret {
                uri: target_uri,
                egress: target_egress,
            },
        ) => {
            let (Some(scope), Some(target)) = (normalize_uri(scope_uri), normalize_uri(target_uri))
            else {
                return false;
            };
            segment_prefix(&scope, &target)
                && egress_covers(scope_egress.as_deref(), target_egress.as_deref())
        }
        (
            ScopedTarget::Config {
                operation: scope_operation,
            },
            ScopedTarget::Config {
                operation: target_operation,
            },
        ) => scope_operation == target_operation,
        (
            ScopedTarget::ChildCount { max: scope_max },
            ScopedTarget::ChildCount { max: target_max },
        ) => target_max <= scope_max,
        (ScopedTarget::None, ScopedTarget::None) => true,
        _ => false,
    }
}

/// Returns whether `scope` is the same path as `target` or a segment prefix.
fn segment_prefix(scope: &str, target: &str) -> bool {
    target == scope || target.starts_with(&format!("{scope}/"))
}

/// Returns whether every requested domain fits the allowed rules.
fn requested_domain_covered(allowed: &[String], requested: &str) -> bool {
    let Some(requested) = normalize_domain(requested) else {
        return false;
    };
    if requested == "*" {
        return allowed
            .iter()
            .any(|rule| normalize_domain(rule).as_deref() == Some("*"));
    }
    match requested.strip_prefix('.') {
        Some(body) => allowed.iter().any(|rule| {
            let Some(rule) = normalize_domain(rule) else {
                return false;
            };
            if rule == "*" || rule == requested {
                return true;
            }
            rule.strip_prefix('.')
                .is_some_and(|suffix| body == suffix || body.ends_with(&format!(".{suffix}")))
        }),
        None => allowed.iter().any(|rule| domain_matches(rule, &requested)),
    }
}

/// Returns whether a granted egress bounds a requested egress.
fn egress_covers(grant: Option<&[String]>, request: Option<&[String]>) -> bool {
    match (grant, request) {
        (None, Some(_)) => false,
        (_, None) => true,
        (Some(grant), Some(request)) => {
            if request.is_empty() {
                return grant.is_empty()
                    || grant
                        .iter()
                        .any(|domain| normalize_domain(domain).as_deref() == Some("*"));
            }
            request
                .iter()
                .all(|requested| requested_domain_covered(grant, requested))
        }
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
    let egress_unbounded = match target {
        ScopedTarget::Secret {
            egress: Some(egress),
            ..
        } => {
            egress.is_empty()
                || egress
                    .iter()
                    .any(|domain| normalize_domain(domain).as_deref() == Some("*"))
        }
        _ => false,
    };
    if !connect_held && !egress_unbounded {
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
        extension_digest: request.extension_digest.clone(),
        config_digest: request.config_digest.clone(),
        expiry_ms: request.now_ms.saturating_add(DEFAULT_APPROVAL_TTL_MS),
        nonce,
    }
}

/// Derives a stable template nonce from the request identity and target.
///
/// The engine is pure, so the nonce cannot be random; it is a canonical
/// fingerprint of the inputs, and `approvals::create_request` assigns the
/// canonical nonce it binds (D10).
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
                Some(egress) if egress.is_empty() => Ok(format!("{uri}|egress=*")),
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
