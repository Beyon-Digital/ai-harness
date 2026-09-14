//! Decision tables, scope rules, confused-deputy intersection, joint risk, and P2.
//!
//! Every case builds an in-memory request and asserts on the returned
//! `Decision`; the engine performs no I/O. Identifiers are parsed from
//! canonical UUIDv7 text, so the suite needs no provider dependency.

use std::collections::BTreeSet;
use std::str::FromStr;

use domain::ids::{ActorId, CapabilityGrantId, DelegationChainId, PrincipalId};
use identity::delegation::{DelegationChain, Hop};
use permissions::capabilities::{parse_capability, supports};
use permissions::policy::{
    DEFAULT_APPROVAL_TTL_MS, Decision, DenyReason, PermissionRequest, ScopedTarget, domain_matches,
    evaluate,
};
use permissions::{Capability, CapabilityAction, CapabilityFamily};
use proptest::prelude::*;

const NOW_MS: i64 = 1_700_000_000_000;

const WORKSPACE_READ: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Read);
const WORKSPACE_WRITE: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Write);
const WORKSPACE_FORK: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Fork);
const WORKSPACE_MERGE: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Merge);
const WORKSPACE_TRANSFER: Capability = Capability::new(
    CapabilityFamily::Workspace,
    CapabilityAction::TransferExclusive,
);
const WORKSPACE_SHARED: Capability = Capability::new(
    CapabilityFamily::Workspace,
    CapabilityAction::SharedCoordinated,
);
const NETWORK_CONNECT: Capability =
    Capability::new(CapabilityFamily::Network, CapabilityAction::Connect);
const SECRET_USE: Capability = Capability::new(CapabilityFamily::Secret, CapabilityAction::Use);
const SECRET_SIGN_OR_ACT: Capability =
    Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct);
const AGENT_SPAWN: Capability = Capability::new(CapabilityFamily::Agent, CapabilityAction::Spawn);
const EXTENSION_INSTALL: Capability =
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Install);
const EXTENSION_ENABLE: Capability =
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Enable);
const EXTENSION_DISABLE: Capability =
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Disable);
const CONFIG_PROPOSE: Capability =
    Capability::new(CapabilityFamily::Config, CapabilityAction::Propose);
const CONFIG_TEST: Capability = Capability::new(CapabilityFamily::Config, CapabilityAction::Test);
const CONFIG_ACTIVATE: Capability =
    Capability::new(CapabilityFamily::Config, CapabilityAction::Activate);
const CONFIG_ROLLBACK: Capability =
    Capability::new(CapabilityFamily::Config, CapabilityAction::Rollback);
const EFFECT_RECONCILE: Capability =
    Capability::new(CapabilityFamily::Effect, CapabilityAction::Reconcile);
const EFFECT_RESOLVE_UNKNOWN: Capability =
    Capability::new(CapabilityFamily::Effect, CapabilityAction::ResolveUnknown);
const RESOURCE_RESERVE: Capability =
    Capability::new(CapabilityFamily::Resource, CapabilityAction::Reserve);

const ALL_CAPABILITIES: [Capability; 20] = [
    WORKSPACE_READ,
    WORKSPACE_WRITE,
    WORKSPACE_FORK,
    WORKSPACE_MERGE,
    WORKSPACE_TRANSFER,
    WORKSPACE_SHARED,
    NETWORK_CONNECT,
    SECRET_USE,
    SECRET_SIGN_OR_ACT,
    AGENT_SPAWN,
    EXTENSION_INSTALL,
    EXTENSION_ENABLE,
    EXTENSION_DISABLE,
    CONFIG_PROPOSE,
    CONFIG_TEST,
    CONFIG_ACTIVATE,
    CONFIG_ROLLBACK,
    EFFECT_RECONCILE,
    EFFECT_RESOLVE_UNKNOWN,
    RESOURCE_RESERVE,
];

/// Renders a canonical lowercase UUIDv7 with the given low bits.
fn id_text(value: u128) -> String {
    format!("0191d2b7-0000-7000-8000-{value:012x}")
}

fn grant_id(value: u128) -> CapabilityGrantId {
    CapabilityGrantId::from_str(&id_text(value)).unwrap()
}

fn actor_id(value: u128) -> ActorId {
    ActorId::from_str(&id_text(value)).unwrap()
}

fn principal_id(value: u128) -> PrincipalId {
    PrincipalId::from_str(&id_text(value)).unwrap()
}

fn chain_id(value: u128) -> DelegationChainId {
    DelegationChainId::from_str(&id_text(value)).unwrap()
}

/// Builds a structurally valid hop whose grant ids back its capabilities.
fn hop(index: u32, capabilities: &[Capability], seed: u128) -> Hop {
    let mut capabilities = capabilities.to_vec();
    capabilities.sort();
    capabilities.dedup();
    let grant_ids = (0..capabilities.len())
        .map(|offset| grant_id(seed + offset as u128))
        .collect();
    Hop {
        hop_index: index,
        actor_id: actor_id(seed),
        run_id: None,
        grant_ids,
        capabilities,
    }
}

fn chain(hops: Vec<Hop>) -> DelegationChain {
    DelegationChain {
        chain_id: chain_id(9_000_000_000),
        hops,
    }
}

/// Builds a root hop holding everything and a tip holding only `capability`.
fn chain_holding(capability: Capability) -> (DelegationChain, CapabilityGrantId) {
    let root = hop(0, &ALL_CAPABILITIES, 1_000_000);
    let tip_grant = grant_id(2_000_000);
    let tip = Hop {
        hop_index: 1,
        actor_id: actor_id(2_000_001),
        run_id: None,
        grant_ids: vec![tip_grant],
        capabilities: vec![capability],
    };
    (chain(vec![root, tip]), tip_grant)
}

fn request(
    chain: DelegationChain,
    tool_capabilities: Option<Vec<Capability>>,
    capability: Capability,
    target: ScopedTarget,
) -> PermissionRequest {
    PermissionRequest {
        principal_id: principal_id(100),
        actor_id: actor_id(101),
        run_id: None,
        chain,
        tool_capabilities,
        capability,
        target,
        now_ms: NOW_MS,
    }
}

fn deny_reason(decision: &Decision) -> DenyReason {
    match decision {
        Decision::Deny { reason } => *reason,
        other => panic!("expected a denial, got {other:?}"),
    }
}

fn approval_draft(decision: &Decision) -> &permissions::policy::ApprovalDraft {
    match decision {
        Decision::RequireApproval { request } => request,
        other => panic!("expected an approval request, got {other:?}"),
    }
}

fn workspace_target(uri: &str, path: Option<&str>) -> ScopedTarget {
    ScopedTarget::Workspace {
        uri: uri.to_string(),
        path: path.map(str::to_string),
    }
}

fn secret_target(uri: &str, egress: Option<Vec<&str>>) -> ScopedTarget {
    ScopedTarget::Secret {
        uri: uri.to_string(),
        egress: egress.map(|list| list.into_iter().map(str::to_string).collect()),
    }
}

#[test]
fn catalogue_parses_every_contract_pair_and_rejects_unknown_values() {
    for capability in ALL_CAPABILITIES {
        let token = capability.to_string();
        assert_eq!(parse_capability(&token).unwrap(), capability, "{token}");
        assert!(supports(&capability), "{token}");
    }
    for token in [
        "network.read",
        "workspace.bogus",
        "bogus.read",
        "read",
        "",
        ".",
        "workspace.",
        ".read",
        "workspace.read.extra",
        "secret.sign",
        "effect.unknown",
    ] {
        assert!(parse_capability(token).is_err(), "{token} must be rejected");
    }
}

#[test]
fn every_contract_capability_allows_on_a_matching_grant_and_target() {
    let workspace = |path: Option<&str>| workspace_target("workspace://acme/proj", path);
    let config = |operation: CapabilityAction| ScopedTarget::Config { operation };
    let cases: [(Capability, ScopedTarget); 20] = [
        (WORKSPACE_READ, workspace(Some("src/lib.rs"))),
        (WORKSPACE_WRITE, workspace(None)),
        (WORKSPACE_FORK, workspace(Some("fork"))),
        (WORKSPACE_MERGE, workspace(Some("merge/heads/main"))),
        (WORKSPACE_TRANSFER, workspace(None)),
        (WORKSPACE_SHARED, workspace(Some("shared/out"))),
        (
            NETWORK_CONNECT,
            ScopedTarget::Network {
                domains: vec!["api.example.com".to_string()],
            },
        ),
        (SECRET_USE, secret_target("secret://prod/api-key", None)),
        (
            SECRET_SIGN_OR_ACT,
            secret_target("secret://prod/signing-key", None),
        ),
        (AGENT_SPAWN, ScopedTarget::ChildCount { max: 4 }),
        (EXTENSION_INSTALL, ScopedTarget::None),
        (EXTENSION_ENABLE, ScopedTarget::None),
        (EXTENSION_DISABLE, ScopedTarget::None),
        (CONFIG_PROPOSE, config(CapabilityAction::Propose)),
        (CONFIG_TEST, config(CapabilityAction::Test)),
        (CONFIG_ACTIVATE, config(CapabilityAction::Activate)),
        (CONFIG_ROLLBACK, config(CapabilityAction::Rollback)),
        (EFFECT_RECONCILE, ScopedTarget::None),
        (EFFECT_RESOLVE_UNKNOWN, ScopedTarget::None),
        (RESOURCE_RESERVE, ScopedTarget::None),
    ];
    for (capability, target) in cases {
        let (chain, tip_grant) = chain_holding(capability);
        let decision = evaluate(&request(chain, None, capability, target));
        assert_eq!(
            decision,
            Decision::Allow {
                grant_refs: vec![tip_grant]
            },
            "{capability}"
        );
    }
}

#[test]
fn workspace_scope_accepts_normalized_roots_and_in_scope_paths() {
    for (uri, path) in [
        ("workspace://acme/proj", None),
        ("workspace://acme/proj/", None),
        ("workspace://acme/proj", Some("src/lib.rs")),
        ("workspace://acme/proj", Some("workspace://acme/proj/src")),
        ("workspace://acme/proj", Some("workspace://acme/proj")),
    ] {
        let (chain, _) = chain_holding(WORKSPACE_READ);
        let decision = evaluate(&request(
            chain,
            None,
            WORKSPACE_READ,
            workspace_target(uri, path),
        ));
        assert!(
            matches!(decision, Decision::Allow { .. }),
            "{uri} {path:?} must be in scope, got {decision:?}"
        );
    }
}

#[test]
fn workspace_scope_rejects_traversal_and_foreign_paths() {
    for (uri, path) in [
        ("workspace://acme/proj", Some("../etc/passwd")),
        ("workspace://acme/proj", Some("src/../../etc/passwd")),
        ("workspace://acme/proj", Some("..")),
        ("workspace://acme/proj", Some("/etc/passwd")),
        ("workspace://acme/proj", Some("")),
        ("workspace://acme/proj", Some("workspace://acme/other/file")),
        (
            "workspace://acme/proj",
            Some("workspace://acme/proj-evil/file"),
        ),
        ("workspace://acme/../etc", None),
        ("workspace://acme/proj", Some("src//lib.rs")),
        ("acme/proj", None),
        ("workspace://", None),
    ] {
        let (chain, _) = chain_holding(WORKSPACE_READ);
        let decision = evaluate(&request(
            chain,
            None,
            WORKSPACE_READ,
            workspace_target(uri, path),
        ));
        assert_eq!(
            deny_reason(&decision),
            DenyReason::OutOfScope,
            "{uri} {path:?} must be out of scope"
        );
    }
}

#[test]
fn network_scope_accepts_exact_suffix_and_wildcard_domains() {
    for domains in [
        vec!["api.example.com"],
        vec!["api.example.com", "cdn.example.com"],
        vec![".example.com"],
        vec!["*"],
        vec!["API.Example.COM"],
    ] {
        let (chain, _) = chain_holding(NETWORK_CONNECT);
        let target = ScopedTarget::Network {
            domains: domains.iter().map(|domain| domain.to_string()).collect(),
        };
        let decision = evaluate(&request(chain, None, NETWORK_CONNECT, target));
        assert!(
            matches!(decision, Decision::Allow { .. }),
            "{domains:?} must be accepted, got {decision:?}"
        );
    }
}

#[test]
fn network_scope_rejects_malformed_domains_and_empty_lists() {
    for domains in [
        vec![],
        vec![""],
        vec!["https://api.example.com"],
        vec!["api.example.com:443"],
        vec!["-bad.example.com"],
        vec!["bad-.example.com"],
        vec!["api..example.com"],
        vec!["api_example.com"],
        vec!["api.example.com/path"],
        vec![" "],
    ] {
        let (chain, _) = chain_holding(NETWORK_CONNECT);
        let target = ScopedTarget::Network {
            domains: domains.iter().map(|domain| domain.to_string()).collect(),
        };
        let decision = evaluate(&request(chain, None, NETWORK_CONNECT, target));
        assert_eq!(
            deny_reason(&decision),
            DenyReason::OutOfScope,
            "{domains:?} must be rejected"
        );
    }
}

#[test]
fn domain_matching_follows_the_documented_exact_and_suffix_rule() {
    assert!(domain_matches("api.example.com", "api.example.com"));
    assert!(domain_matches("api.example.com", "API.Example.COM"));
    assert!(!domain_matches("api.example.com", "evil.example.com"));
    assert!(!domain_matches("api.example.com", "api.example.com.evil"));
    assert!(domain_matches(".example.com", "example.com"));
    assert!(domain_matches(".example.com", "api.example.com"));
    assert!(!domain_matches(".example.com", "badexample.com"));
    assert!(domain_matches("*", "anything.example"));
    assert!(!domain_matches(
        "api.example.com",
        "https://api.example.com"
    ));
}

#[test]
fn secret_scope_requires_a_uri_and_well_formed_egress() {
    for target in [
        secret_target("api-key", None),
        secret_target("secret://", None),
        secret_target("secret://prod/../other", None),
        secret_target("secret://prod/api-key", Some(vec![])),
        secret_target(
            "secret://prod/api-key",
            Some(vec!["https://api.example.com"]),
        ),
        secret_target("secret://prod/api-key", Some(vec![" "])),
    ] {
        let (chain, _) = chain_holding(SECRET_USE);
        let decision = evaluate(&request(chain, None, SECRET_USE, target));
        assert_eq!(
            deny_reason(&decision),
            DenyReason::OutOfScope,
            "malformed secret scope must be denied"
        );
    }
}

#[test]
fn scoped_targets_must_match_the_capability_family() {
    let mismatches = [
        (
            WORKSPACE_READ,
            ScopedTarget::Network {
                domains: vec!["api.example.com".to_string()],
            },
        ),
        (
            NETWORK_CONNECT,
            workspace_target("workspace://acme/proj", None),
        ),
        (SECRET_USE, workspace_target("workspace://acme/proj", None)),
        (SECRET_USE, ScopedTarget::None),
        (AGENT_SPAWN, ScopedTarget::None),
        (AGENT_SPAWN, ScopedTarget::ChildCount { max: 0 }),
        (EXTENSION_INSTALL, ScopedTarget::ChildCount { max: 2 }),
        (
            CONFIG_ACTIVATE,
            ScopedTarget::Config {
                operation: CapabilityAction::Test,
            },
        ),
        (
            CONFIG_ACTIVATE,
            ScopedTarget::Config {
                operation: CapabilityAction::Read,
            },
        ),
        (
            EFFECT_RECONCILE,
            ScopedTarget::Network {
                domains: vec!["api.example.com".to_string()],
            },
        ),
        (
            RESOURCE_RESERVE,
            secret_target("secret://prod/api-key", None),
        ),
        (WORKSPACE_READ, ScopedTarget::None),
    ];
    for (capability, target) in mismatches {
        let (chain, _) = chain_holding(capability);
        let decision = evaluate(&request(chain, None, capability, target));
        assert_eq!(
            deny_reason(&decision),
            DenyReason::OutOfScope,
            "{capability} with a mismatched target must be out of scope"
        );
    }
}

#[test]
fn an_empty_chain_denies_with_no_grant() {
    let decision = evaluate(&request(
        chain(vec![]),
        None,
        WORKSPACE_READ,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
}

#[test]
fn a_missing_capability_denies_with_no_grant() {
    let (chain, _) = chain_holding(WORKSPACE_READ);
    let decision = evaluate(&request(
        chain,
        None,
        WORKSPACE_WRITE,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
}

#[test]
fn confused_deputy_intersects_child_grants_with_the_tool_allow_list() {
    let root = hop(0, &[WORKSPACE_READ, WORKSPACE_WRITE], 1_000_000);
    let child = hop(1, &[WORKSPACE_READ], 2_000_000);
    let privileged_helper = Some(vec![WORKSPACE_WRITE]);
    let decision = evaluate(&request(
        chain(vec![root, child]),
        privileged_helper,
        WORKSPACE_WRITE,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
}

#[test]
fn tool_restriction_denies_capabilities_outside_the_tool_allow_list() {
    let (chain, _) = chain_holding(WORKSPACE_WRITE);
    let decision = evaluate(&request(
        chain,
        Some(vec![WORKSPACE_READ]),
        WORKSPACE_WRITE,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::ToolRestriction);
}

#[test]
fn tool_allow_list_does_not_widen_authority() {
    let (chain, _) = chain_holding(WORKSPACE_WRITE);
    let decision = evaluate(&request(
        chain,
        Some(vec![WORKSPACE_WRITE]),
        WORKSPACE_WRITE,
        workspace_target("workspace://acme/proj", None),
    ));
    assert!(matches!(decision, Decision::Allow { .. }));
}

#[test]
fn a_tip_that_exceeds_its_ancestor_denies_with_ancestor_constraint() {
    let root = hop(0, &[WORKSPACE_READ], 1_000_000);
    let over_broad_tip = hop(1, &[WORKSPACE_READ, WORKSPACE_WRITE], 2_000_000);
    let decision = evaluate(&request(
        chain(vec![root, over_broad_tip]),
        None,
        WORKSPACE_WRITE,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::AncestorConstraint);
}

#[test]
fn an_invalid_chain_shape_denies_with_no_grant() {
    let root = hop(0, &[WORKSPACE_READ], 1_000_000);
    let mut gapped = hop(1, &[WORKSPACE_READ], 2_000_000);
    gapped.hop_index = 5;
    let decision = evaluate(&request(
        chain(vec![root, gapped]),
        None,
        WORKSPACE_READ,
        workspace_target("workspace://acme/proj", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
}

#[test]
fn an_off_contract_capability_denies_as_unsupported() {
    let rogue = Capability::new(CapabilityFamily::Network, CapabilityAction::Read);
    let (chain, _) = chain_holding(rogue);
    let decision = evaluate(&request(
        chain,
        None,
        rogue,
        ScopedTarget::Network {
            domains: vec!["api.example.com".to_string()],
        },
    ));
    assert_eq!(deny_reason(&decision), DenyReason::Unsupported);
}

#[test]
fn secret_use_with_egress_requires_approval_with_a_deterministic_draft() {
    let (chain, _) = chain_holding(SECRET_USE);
    let request = request(
        chain,
        None,
        SECRET_USE,
        secret_target("secret://prod/api-key", Some(vec!["api.example.com"])),
    );
    let first = evaluate(&request);
    let second = evaluate(&request);
    assert_eq!(
        first, second,
        "identical inputs must yield identical drafts"
    );
    let draft = approval_draft(&first);
    assert_eq!(draft.operation, "secret.use");
    assert_eq!(draft.target, "secret://prod/api-key|egress=api.example.com");
    assert_eq!(draft.capabilities, vec![SECRET_USE]);
    assert_eq!(draft.expiry_ms, NOW_MS + DEFAULT_APPROVAL_TTL_MS);
    assert!(!draft.nonce.is_empty());
    assert!(draft.extension_digest.is_none());
    assert!(draft.config_digest.is_none());
}

#[test]
fn secret_sign_or_act_with_held_network_connect_requires_approval() {
    let root = hop(0, &ALL_CAPABILITIES, 1_000_000);
    let tip = hop(1, &[NETWORK_CONNECT, SECRET_SIGN_OR_ACT], 2_000_000);
    let decision = evaluate(&request(
        chain(vec![root, tip]),
        None,
        SECRET_SIGN_OR_ACT,
        secret_target("secret://prod/signing-key", None),
    ));
    let draft = approval_draft(&decision);
    assert_eq!(
        draft.capabilities,
        vec![NETWORK_CONNECT, SECRET_SIGN_OR_ACT]
    );
    assert_eq!(draft.target, "secret://prod/signing-key");
}

#[test]
fn secret_use_without_egress_or_connect_allows() {
    let (chain, tip_grant) = chain_holding(SECRET_USE);
    let decision = evaluate(&request(
        chain,
        None,
        SECRET_USE,
        secret_target("secret://prod/api-key", None),
    ));
    assert_eq!(
        decision,
        Decision::Allow {
            grant_refs: vec![tip_grant]
        }
    );
}

#[test]
fn joint_risk_respects_the_tool_intersection() {
    let root = hop(0, &ALL_CAPABILITIES, 1_000_000);
    let tip = hop(1, &[NETWORK_CONNECT, SECRET_USE], 2_000_000);
    let decision = evaluate(&request(
        chain(vec![root, tip]),
        Some(vec![SECRET_USE]),
        SECRET_USE,
        secret_target("secret://prod/api-key", None),
    ));
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "a tool without network authority cannot trigger the connect joint rule"
    );
}

#[test]
fn denial_takes_precedence_over_joint_risk() {
    let decision = evaluate(&request(
        chain(vec![]),
        None,
        SECRET_USE,
        secret_target("secret://prod/api-key", Some(vec!["api.example.com"])),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
}

#[test]
fn denial_reasons_carry_no_target_content() {
    let decision = evaluate(&request(
        chain(vec![]),
        None,
        SECRET_USE,
        secret_target("secret://prod/payments-key", None),
    ));
    assert_eq!(deny_reason(&decision), DenyReason::NoGrant);
    let rendered = format!("{decision:?}");
    assert!(!rendered.contains("secret://"));
    assert!(!rendered.contains("payments-key"));
}

fn capability_strategy() -> impl Strategy<Value = Capability> {
    prop::sample::select(ALL_CAPABILITIES.to_vec())
}

fn chain_strategy() -> impl Strategy<Value = DelegationChain> {
    prop::collection::vec(prop::collection::vec(capability_strategy(), 0..=6), 0..=3).prop_map(
        |layers| {
            let mut hops = Vec::new();
            let mut parent: Option<BTreeSet<Capability>> = None;
            for (index, layer) in layers.iter().enumerate() {
                let held: BTreeSet<Capability> = match &parent {
                    Some(parent) => layer
                        .iter()
                        .copied()
                        .filter(|capability| parent.contains(capability))
                        .collect(),
                    None => layer.iter().copied().collect(),
                };
                let capabilities: Vec<Capability> = held.iter().copied().collect();
                hops.push(hop(
                    u32::try_from(index).unwrap(),
                    &capabilities,
                    1_000_000 + (index as u128) * 100,
                ));
                parent = Some(held);
            }
            chain(hops)
        },
    )
}

fn target_strategy() -> impl Strategy<Value = ScopedTarget> {
    prop_oneof![
        Just(ScopedTarget::None),
        ("[a-z]{1,8}", prop::option::of("[a-z/]{0,10}")).prop_map(|(name, path)| {
            ScopedTarget::Workspace {
                uri: format!("workspace://{name}"),
                path,
            }
        }),
        prop::collection::vec("[a-z.]{1,12}", 0..=3)
            .prop_map(|domains| ScopedTarget::Network { domains }),
        (
            "[a-z]{1,6}",
            prop::option::of(prop::collection::vec("[a-z.]{1,12}", 1..=2)),
        )
            .prop_map(|(name, egress)| ScopedTarget::Secret {
                uri: format!("secret://{name}"),
                egress,
            }),
        (1u32..8).prop_map(|max| ScopedTarget::ChildCount { max }),
        capability_strategy().prop_map(|capability| ScopedTarget::Config {
            operation: capability.action,
        }),
    ]
}

fn request_strategy() -> impl Strategy<Value = PermissionRequest> {
    (
        chain_strategy(),
        prop::option::of(prop::collection::vec(capability_strategy(), 0..=6)),
        capability_strategy(),
        0usize..8,
        target_strategy(),
        0i64..1_000_000_000,
    )
        .prop_map(|(chain, tool, fallback, index, target, now_ms)| {
            let capability = match chain.hops.last() {
                Some(tip) if !tip.capabilities.is_empty() => {
                    tip.capabilities[index % tip.capabilities.len()]
                }
                _ => fallback,
            };
            let tool_capabilities = tool.map(|mut tool| {
                tool.sort();
                tool.dedup();
                tool
            });
            PermissionRequest {
                principal_id: principal_id(7),
                actor_id: actor_id(8),
                run_id: None,
                chain,
                tool_capabilities,
                capability,
                target,
                now_ms,
            }
        })
}

proptest! {
    #[test]
    fn p2_every_input_yields_exactly_one_deterministic_variant(request in request_strategy()) {
        let first = evaluate(&request);
        let second = evaluate(&request);
        prop_assert_eq!(&first, &second, "evaluate must be deterministic");

        match &first {
            Decision::Allow { grant_refs } => {
                prop_assert!(!grant_refs.is_empty(), "allow must cite at least one grant");
                let cited: BTreeSet<CapabilityGrantId> = grant_refs.iter().copied().collect();
                prop_assert_eq!(cited.len(), grant_refs.len(), "grant refs must be unique");
                let tip_grants: BTreeSet<CapabilityGrantId> = request
                    .chain
                    .hops
                    .last()
                    .map(|tip| tip.grant_ids.iter().copied().collect())
                    .unwrap_or_default();
                prop_assert!(cited.is_subset(&tip_grants), "allow must cite the tip's own grants");
            }
            Decision::Deny { .. } => {}
            Decision::RequireApproval { request: draft } => {
                prop_assert!(!draft.nonce.is_empty());
                prop_assert!(draft.capabilities.contains(&request.capability));
            }
        }
    }

    #[test]
    fn p2_tool_allow_lists_never_widen_authority(request in request_strategy()) {
        let unrestricted = evaluate(&PermissionRequest {
            tool_capabilities: None,
            ..request.clone()
        });
        let empty_tool = evaluate(&PermissionRequest {
            tool_capabilities: Some(Vec::new()),
            ..request.clone()
        });
        prop_assert!(
            !matches!(empty_tool, Decision::Allow { .. }),
            "an empty tool allow list cannot allow"
        );
        if matches!(evaluate(&request), Decision::Allow { .. }) {
            prop_assert!(
                !matches!(unrestricted, Decision::Deny { .. }),
                "dropping the tool allow list cannot deny an operation the tool allowed"
            );
        }
    }
}
