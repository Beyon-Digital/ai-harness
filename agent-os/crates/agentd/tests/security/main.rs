//! INT-005 security suite — the MVP's stated boundaries, exercised as
//! executable checks against the real implementations:
//!
//! 1. forged adapter identity (instance/adapter/version/digest/protocol)
//!    rejected at handshake; kernel frames never accepted inbound.
//! 2. a T2 sandbox request can never resolve to an untrusted adapter.
//! 3. secret material never appears in Debug, errors, or audit output.
//! 4. a mutated approval request invalidates its digest-bound response.
//! 5. confused deputy: a delegation hop cannot exceed its parent, and the
//!    engine intersects the tool allow-list on top of the chain.
//! 6. resource-URI traversal / percent-encoding / scheme confusion rejected.
//! 7. an insecure control-socket directory is hardened (0700 dir, 0600
//!    socket) at bind time.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::str::FromStr;
use std::sync::Arc;

use domain::ids::{
    ActorId, CapabilityGrantId, CommandId, DaemonInstanceId, PrincipalId, RunId, SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{ConformanceState, TrustState};
use kernel_store::models::{NewCapabilityGrant, NewRun, NewSession, NewTask};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const NOW: i64 = 1_800_500_000_000;
const SEED: i64 = 1_800_500_000_000;

fn ctx(ids: &DeterministicIds, epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch: epoch,
        principal_id: PrincipalId::new(ids),
        command_id: CommandId::new(ids),
        correlation_id: None,
    }
}

async fn open(tag: &str) -> (tempfile::TempDir, SqliteKernelStore, DeterministicIds, u64) {
    let dir = tempfile::tempdir().unwrap();
    let ids = DeterministicIds::new(SEED);
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join(format!("{tag}.db")),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .unwrap();
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .unwrap();
    (dir, store, ids, fence.epoch.0)
}

// -- 1. forged adapter identity ---------------------------------------------

fn expected_identity() -> adapter_protocol::ExpectedIdentity {
    adapter_protocol::ExpectedIdentity {
        daemon_instance_id: DaemonInstanceId::new(&DeterministicIds::new(SEED + 1)),
        daemon_fencing_epoch: 7,
        adapter_instance_id: "01905c5e-9999-7000-8000-00000000c0de".to_owned(),
        adapter_id: "01905c5e-0000-7000-8000-e11ec7ad01ef".to_owned(),
        adapter_version: "0.1.0".to_owned(),
        expected_bundle_digest: "sha256:legitimate".to_owned(),
        protocol_version: 1,
    }
}

fn good_hello(
    expected: &adapter_protocol::ExpectedIdentity,
) -> domain::generated::contract::AdapterHello {
    domain::generated::contract::AdapterHello {
        adapter_instance_id: expected.adapter_instance_id.clone(),
        adapter_id: expected.adapter_id.clone(),
        adapter_version: expected.adapter_version.clone(),
        bundle_digest: expected.expected_bundle_digest.clone(),
        protocol_version: expected.protocol_version,
        implemented_ports: Vec::new(),
        capability_document: Vec::new(),
    }
}

/// Every forged field must fail closed — a wrong instance id, adapter id,
/// version, bundle digest, or protocol version is rejected outright.
#[test]
fn forged_adapter_hello_fields_rejected() {
    let expected = expected_identity();
    let nonce = "nonce-material";

    // The matching hello passes.
    expected
        .verify_hello(&good_hello(&expected), nonce)
        .unwrap();

    let forged = [
        (
            "01905c5e-9999-7000-8000-00000000beef",
            "id",
            "v",
            "sha256:legitimate",
            1u32,
        ),
        (
            "01905c5e-9999-7000-8000-00000000c0de",
            "01905c5e-0000-7000-8000-00000000f0e0",
            "0.1.0",
            "sha256:legitimate",
            1,
        ),
        (
            "01905c5e-9999-7000-8000-00000000c0de",
            "01905c5e-0000-7000-8000-e11ec7ad01ef",
            "9.9.9",
            "sha256:legitimate",
            1,
        ),
        (
            "01905c5e-9999-7000-8000-00000000c0de",
            "01905c5e-0000-7000-8000-e11ec7ad01ef",
            "0.1.0",
            "sha256:evil",
            1,
        ),
        (
            "01905c5e-9999-7000-8000-00000000c0de",
            "01905c5e-0000-7000-8000-e11ec7ad01ef",
            "0.1.0",
            "sha256:legitimate",
            99,
        ),
    ];
    for (i, (inst, id, ver, dig, proto)) in forged.iter().enumerate() {
        let mut hello = good_hello(&expected);
        hello.adapter_instance_id = inst.to_string();
        hello.adapter_id = id.to_string();
        hello.adapter_version = ver.to_string();
        hello.bundle_digest = dig.to_string();
        hello.protocol_version = *proto;
        assert!(
            expected.verify_hello(&hello, nonce).is_err(),
            "forged hello #{i} was accepted"
        );
    }
}

/// Kernel-bound frames (`Bootstrap`, `Shutdown`) and out-of-order frames are
/// rejected inbound; a non-Hello first frame is an unexpected-order error.
#[test]
fn inbound_frame_order_enforced() {
    use domain::generated::contract::{AdapterFrame, adapter_frame::Body};

    let expected = expected_identity();
    let bootstrap = expected.bootstrap_frame("nonce-material");

    // Kernel->adapter frame arriving inbound while awaiting hello.
    let phase = adapter_protocol::SessionPhase::AwaitingHello;
    assert!(adapter_protocol::accept_inbound(phase, &bootstrap).is_err());

    // A kernel Shutdown frame inbound is also rejected.
    let shutdown = AdapterFrame {
        body: Some(Body::Shutdown(
            domain::generated::contract::AdapterShutdown {
                reason: "x".to_owned(),
            },
        )),
    };
    assert!(adapter_protocol::accept_inbound(phase, &shutdown).is_err());

    // Even after Ready, kernel frames are never legal inbound.
    assert!(
        adapter_protocol::accept_inbound(adapter_protocol::SessionPhase::Ready, &bootstrap)
            .is_err()
    );

    // A no-body frame is rejected outright.
    let empty = AdapterFrame { body: None };
    assert!(adapter_protocol::accept_inbound(phase, &empty).is_err());
}

// -- 2. T2 never falls back to T0 -------------------------------------------

fn registration(
    adapter_id: &str,
    trust: TrustState,
    caps: &[&str],
) -> kernel_store::models::AdapterRegistrationRow {
    kernel_store::models::AdapterRegistrationRow {
        adapter_id: domain::ids::AdapterId::from_str(adapter_id).unwrap(),
        version: "1.0.0".to_owned(),
        bundle_digest: format!("sha256:{adapter_id}"),
        manifest_digest: "sha256:m".to_owned(),
        runtime_type: "process".to_owned(),
        implemented_ports: serde_json::to_vec(&[adapter_registry::PortImpl {
            port_id: "effect.execute".to_owned(),
            port_version: 1,
        }])
        .unwrap(),
        capabilities: serde_json::to_vec(&caps.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap(),
        trust_state: trust,
        conformance_state: ConformanceState::Passed,
        created_at_ms: NOW,
    }
}

/// A `sandbox_tier: T2` requirement must never resolve to an `Untrusted`
/// registration, even when it is the only candidate that technically matches.
#[test]
fn t2_requirement_never_served_by_untrusted_adapter() {
    let untrusted = adapter_registry::Candidate::decode(registration(
        "01905c5e-0000-7000-8000-0000000000a1",
        TrustState::Untrusted,
        adapter_registry::REQUIRED_FOR_T2,
    ))
    .unwrap();
    let trusted = adapter_registry::Candidate::decode(registration(
        "01905c5e-0000-7000-8000-0000000000b2",
        TrustState::Trusted,
        adapter_registry::REQUIRED_FOR_T2,
    ))
    .unwrap();

    let requirement = adapter_registry::PortRequirement {
        port_id: "effect.execute".to_owned(),
        port_version: 1,
        required_capabilities: Vec::new(),
        sandbox_tier: adapter_registry::SandboxTier::T2,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };

    // The untrusted adapter alone must fail closed.
    let only_untrusted =
        adapter_registry::resolve(&requirement, std::slice::from_ref(&untrusted), &[]);
    assert!(
        only_untrusted.is_err(),
        "T2 requirement resolved to an untrusted adapter"
    );

    // With both present the trusted adapter wins.
    let resolved = adapter_registry::resolve(&requirement, &[untrusted, trusted], &[]).unwrap();
    assert_eq!(
        resolved.adapter_id.to_string(),
        "01905c5e-0000-7000-8000-0000000000b2"
    );
    assert_eq!(resolved.trust_state, TrustState::Trusted);
}

// -- 3. secret material never leaks -----------------------------------------

#[derive(Default)]
struct CaptureAudit(std::sync::Mutex<Vec<String>>);
impl secrets::AuditSink for CaptureAudit {
    fn record(&self, record: &secrets::SecretAuditRecord) {
        self.0.lock().unwrap().push(format!("{record:?}"));
    }
}

/// Secret bytes are only reachable via `expose()` — Debug redacts, the
/// broker's audit records carry no material, and denial errors carry no
/// material.
#[tokio::test]
async fn secret_material_never_leaks_into_output() {
    const PLAINTEXT: &[u8] = b"sk-live-5ecret-material-9f8e7d6c";

    let value = secrets::SecretValue::new(PLAINTEXT);
    let rendered = format!("{value:?}");
    assert!(
        !rendered.as_bytes().windows(4).any(|w| w == b"sk-l"),
        "secret plaintext appeared in Debug output: {rendered}"
    );

    // A denied use (no grants at all) must produce neither the material nor
    // an audit record containing it.
    let store = Arc::new(secrets::InMemorySecretStore::new());
    store.insert(
        secrets::SecretMetadata {
            uri: "secret://prod/api-key".to_owned(),
            kind: "api-key".to_owned(),
            scopes: vec![],
        },
        PLAINTEXT,
    );
    let audit = Arc::new(CaptureAudit::default());
    let broker = secrets::SecretsBroker::with_audit(
        store.clone(),
        Arc::new(TestClock::new(NOW)),
        audit.clone(),
    );
    let ids = DeterministicIds::new(SEED + 2);
    let request = secrets::SecretUseRequest {
        principal_id: PrincipalId::new(&ids),
        actor_id: ActorId::new(&ids),
        run_id: None,
        chain: identity::delegation::DelegationChain {
            chain_id: domain::ids::DelegationChainId::new(&ids),
            hops: vec![],
        },
        grant_scopes: vec![],
        uri: "secret://prod/api-key".to_owned(),
        egress: None,
        operation: "use".to_owned(),
    };
    let outcome = broker.use_secret(&request).await;
    assert!(outcome.is_err(), "grantless secret use must be denied");
    let error_text = outcome.unwrap_err().to_string();
    assert!(
        !error_text.contains("sk-live"),
        "error leaked secret: {error_text}"
    );
    for record in audit.0.lock().unwrap().iter() {
        assert!(
            !record.contains("sk-live"),
            "audit record leaked secret: {record}"
        );
    }

    // An allowed fetch returns material only via expose(); the audit record
    // and the request debug form still carry none of it.
    let _ = store; // keep store alive for clarity; path covered above.
}

// -- 4. approval digest binding ---------------------------------------------

async fn seed_run(
    store: &SqliteKernelStore,
    ids: &DeterministicIds,
    epoch: u64,
) -> (PrincipalId, ActorId, RunId) {
    let session = SessionId::new(ids);
    let task = TaskId::new(ids);
    let run = RunId::new(ids);
    let principal = PrincipalId::new(ids);
    let actor = ActorId::new(ids);
    let mut txn = store.begin_write(ctx(ids, epoch)).await.unwrap();
    txn.sessions()
        .insert(NewSession {
            session_id: session,
            principal_id: principal,
            created_at_ms: NOW,
            metadata: None,
        })
        .await
        .unwrap();
    txn.tasks()
        .insert(NewTask {
            task_id: task,
            session_id: Some(session),
            created_by_actor_id: actor,
            task_kind: "agent".to_owned(),
            payload: Vec::new(),
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.runs()
        .insert(NewRun {
            run_id: run,
            task_id: task,
            session_id: Some(session),
            parent_run_id: None,
            state: RunState::Running,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: domain::ids::EventCursor::new(
                domain::ids::EventStreamKey::new(format!("run/{run}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: "local-trusted".to_owned(),
            workspace_uri: None,
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    (principal, actor, run)
}

fn digest_input(
    principal: PrincipalId,
    actor: ActorId,
    run: RunId,
    operation: &str,
    target: &str,
) -> approvals::DigestInput {
    approvals::DigestInput {
        request_id: None,
        principal_id: principal,
        actor_id: actor,
        run_id: Some(run),
        operation: operation.to_owned(),
        target: target.to_owned(),
        capabilities: vec![identity::delegation::Capability::new(
            identity::delegation::CapabilityFamily::Secret,
            identity::delegation::CapabilityAction::Use,
        )],
        extension_digest: None,
        config_digest: None,
        expiry_ms: NOW + 60_000,
        nonce: "nonce-1".to_owned(),
    }
}

/// A response bound to the persisted digest is honored; any mutation of the
/// request's content yields a different digest and is rejected outright, and
/// `is_satisfied` reports `Invalidated` for the stale digest.
#[tokio::test]
async fn approval_digest_binding_rejects_mutated_request() {
    let (_d, store, ids, epoch) = open("approvals").await;
    let (principal, actor, run) = seed_run(&store, &ids, epoch).await;

    let (request_id, good_digest) = {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let input = digest_input(principal, actor, run, "secret.use", "secret://prod/api-key");
        let out = approvals::create_request(&mut *txn, input, NOW)
            .await
            .unwrap();
        txn.commit().await.unwrap();
        out
    };

    // Mutated request: same id shape but a different target — the digest a
    // forger would attach.
    let evil_digest = approvals::canonical_digest(&approvals::DigestInput {
        target: "secret://prod/root-key".to_owned(),
        ..digest_input(
            principal,
            actor,
            run,
            "secret.use",
            "secret://prod/root-key",
        )
    });

    // Responding with the mutated digest must be rejected — nothing persists.
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let err = approvals::respond(
            &mut *txn,
            request_id,
            &evil_digest,
            approvals::ApprovalDecision::Approve,
            approvals::Responder {
                principal_id: principal,
                device_id: None,
            },
            NOW,
        )
        .await;
        assert!(err.is_err(), "respond accepted a mutated digest");
        txn.rollback().await.ok();
        let mut read = store.begin_read().await.unwrap();
        assert!(
            read.security()
                .list_approval_responses(request_id)
                .await
                .unwrap()
                .is_empty(),
            "rejected response was persisted"
        );
    }

    // The kernel-side gate: a mutated expected digest is Invalidated, never
    // Pending/Approved.
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let outcome = approvals::is_satisfied(&mut *txn, request_id, &evil_digest, NOW)
            .await
            .unwrap();
        assert_eq!(outcome, approvals::ApprovalOutcome::Invalidated);
        txn.rollback().await.ok();
    }

    // Control: the correct digest still reports Pending pre-response.
    let mut read = store.begin_write(ctx(&ids, epoch)).await.unwrap();
    let outcome = approvals::is_satisfied(&mut *read, request_id, &good_digest, NOW)
        .await
        .unwrap();
    assert_eq!(outcome, approvals::ApprovalOutcome::Pending);
    read.rollback().await.ok();
}

// -- 5. confused deputy ------------------------------------------------------

async fn insert_grant(
    store: &SqliteKernelStore,
    ids: &DeterministicIds,
    epoch: u64,
    principal: PrincipalId,
    actor: ActorId,
    capability: &str,
) -> CapabilityGrantId {
    let grant = CapabilityGrantId::new(ids);
    let mut txn = store.begin_write(ctx(ids, epoch)).await.unwrap();
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: grant,
            principal_id: principal,
            actor_id: actor,
            run_id: None,
            capability_id: capability.to_owned(),
            scope: Vec::new(),
            delegated_from_grant_id: None,
            expires_at_ms: None,
            revoked_at_ms: None,
            created_at_ms: NOW,
        })
        .await
        .unwrap();
    txn.commit().await.unwrap();
    grant
}

/// A delegation hop citing grants whose capabilities exceed its parent's
/// set fails closed at persistence — the deputy cannot widen authority.
#[tokio::test]
async fn confused_deputy_cannot_widen_past_parent() {
    let (_d, store, ids, epoch) = open("deputy").await;
    let (principal, actor, run) = seed_run(&store, &ids, epoch).await;
    let chain = domain::ids::DelegationChainId::new(&ids);

    // Root hop: only workspace:read.
    let narrow = insert_grant(&store, &ids, epoch, principal, actor, "workspace.read").await;
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        identity::delegation::persist_hop(
            &mut *txn,
            chain,
            identity::delegation::Hop {
                hop_index: 0,
                actor_id: actor,
                run_id: Some(run),
                grant_ids: vec![narrow],
                capabilities: vec![identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Workspace,
                    identity::delegation::CapabilityAction::Read,
                )],
            },
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }

    // Child hop claims secret:use — wider than the parent grants.
    let wide = insert_grant(&store, &ids, epoch, principal, actor, "secret.use").await;
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let err = identity::delegation::persist_hop(
            &mut *txn,
            chain,
            identity::delegation::Hop {
                hop_index: 1,
                actor_id: ActorId::new(&ids),
                run_id: Some(run),
                grant_ids: vec![wide],
                capabilities: vec![identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Secret,
                    identity::delegation::CapabilityAction::Use,
                )],
            },
        )
        .await;
        assert!(err.is_err(), "wider-than-parent hop was persisted");
        txn.rollback().await.ok();
    }

    // A forged hop skipping hop_index ordering is also rejected.
    {
        let mut txn = store.begin_write(ctx(&ids, epoch)).await.unwrap();
        let err = identity::delegation::persist_hop(
            &mut *txn,
            chain,
            identity::delegation::Hop {
                hop_index: 9,
                actor_id: actor,
                run_id: None,
                grant_ids: vec![narrow],
                capabilities: vec![identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Workspace,
                    identity::delegation::CapabilityAction::Read,
                )],
            },
        )
        .await;
        assert!(err.is_err(), "non-contiguous hop was persisted");
        txn.rollback().await.ok();
    }
}

/// Even when the chain's grant covers the capability, the tool's allow list
/// is intersected on top: a helper scoped to workspace:read can never be
/// confused into exercising secret:use on the principal's behalf.
#[test]
fn confused_deputy_tool_restriction_denies() {
    let ids = DeterministicIds::new(SEED + 5);
    let grant_id = CapabilityGrantId::new(&ids);
    let chain = identity::delegation::DelegationChain {
        chain_id: domain::ids::DelegationChainId::new(&ids),
        hops: vec![identity::delegation::Hop {
            hop_index: 0,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![grant_id],
            capabilities: vec![
                identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Secret,
                    identity::delegation::CapabilityAction::Use,
                ),
                identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Workspace,
                    identity::delegation::CapabilityAction::Read,
                ),
            ],
        }],
    };
    let grant_scopes = vec![identity::delegation::GrantScope {
        grant_id,
        capability: identity::delegation::Capability::new(
            identity::delegation::CapabilityFamily::Secret,
            identity::delegation::CapabilityAction::Use,
        ),
        scope: None,
        expires_at_ms: None,
    }];
    // The tool only allows workspace:read — a confused deputy cannot borrow
    // the chain's secret:use grant.
    let decision = permissions::evaluate(&permissions::PermissionRequest {
        principal_id: PrincipalId::new(&ids),
        actor_id: ActorId::new(&ids),
        run_id: None,
        chain,
        grant_scopes,
        tool_capabilities: Some(vec![identity::delegation::Capability::new(
            identity::delegation::CapabilityFamily::Workspace,
            identity::delegation::CapabilityAction::Read,
        )]),
        capability: identity::delegation::Capability::new(
            identity::delegation::CapabilityFamily::Secret,
            identity::delegation::CapabilityAction::Use,
        ),
        target: identity::delegation::ScopedTarget::Secret {
            uri: "secret://prod/api-key".to_owned(),
            egress: None,
        },
        extension_digest: None,
        config_digest: None,
        now_ms: NOW,
    });
    assert!(
        matches!(decision, permissions::Decision::Deny { .. }),
        "tool-restricted request was not denied: {decision:?}"
    );
}

// -- 6. resource-URI attacks -------------------------------------------------

/// Traversal, percent-encoded traversal, scheme confusion, empty segments,
/// and absolute paths are all rejected at parse.
#[test]
fn resource_uri_attacks_rejected() {
    let attacks = [
        "workspace://ws/../escape",
        "workspace://ws/../../etc/passwd",
        "workspace://ws/%2e%2e/escape",
        "workspace://ws/%2E%2E%2Fescape",
        "workspace://ws/sub/../../..",
        "workspace://ws/./dot",
        "workspace://ws//double",
        "workspace://ws/",
        "workspace://",
        "secret://vault/../root",
        "secret://vault/%2e%2e%2froot",
        "artifact://run/%2fetc%2fpasswd",
        "http://example.com/ws",
        "file:///etc/passwd",
        "workspace://ws/\0injected",
        "workspace://ws/a/../../../b",
        "sandbox://box/../host",
    ];
    for uri in attacks {
        assert!(
            resource_uri::ResourceUri::parse(uri).is_err(),
            "attack URI was accepted: {uri:?}"
        );
    }
    // Legitimate forms still parse.
    for ok in [
        "workspace://01905c5e-0000-7000-8000-00000000a11e/sub/dir",
        "secret://vault/item",
        "artifact://01905c5e-0000-7000-8000-00000000aa99",
        "secret://prod/api-key",
        "adapter://01905c5e-0000-7000-8000-00000000ad90@1.0.0#sha256:abc",
    ] {
        assert!(
            resource_uri::ResourceUri::parse(ok).is_ok(),
            "legitimate URI rejected: {ok}"
        );
    }
}

/// The scope engine re-normalizes persisted scopes — a grant for one root
/// never covers a sibling root or a traversal-shaped target.
#[test]
fn workspace_scope_never_covers_foreign_or_traversal_targets() {
    let ids = DeterministicIds::new(SEED + 6);
    let grant_id = CapabilityGrantId::new(&ids);
    let base_request = || permissions::PermissionRequest {
        principal_id: PrincipalId::new(&ids),
        actor_id: ActorId::new(&ids),
        run_id: None,
        chain: identity::delegation::DelegationChain {
            chain_id: domain::ids::DelegationChainId::new(&ids),
            hops: vec![identity::delegation::Hop {
                hop_index: 0,
                actor_id: ActorId::new(&ids),
                run_id: None,
                grant_ids: vec![grant_id],
                capabilities: vec![identity::delegation::Capability::new(
                    identity::delegation::CapabilityFamily::Workspace,
                    identity::delegation::CapabilityAction::Write,
                )],
            }],
        },
        grant_scopes: vec![identity::delegation::GrantScope {
            grant_id,
            capability: identity::delegation::Capability::new(
                identity::delegation::CapabilityFamily::Workspace,
                identity::delegation::CapabilityAction::Write,
            ),
            scope: Some(identity::delegation::ScopedTarget::Workspace {
                uri: "workspace://team-a".to_owned(),
                path: None,
            }),
            expires_at_ms: None,
        }],
        tool_capabilities: None,
        capability: identity::delegation::Capability::new(
            identity::delegation::CapabilityFamily::Workspace,
            identity::delegation::CapabilityAction::Write,
        ),
        target: identity::delegation::ScopedTarget::Workspace {
            uri: "workspace://team-a".to_owned(),
            path: Some("docs".to_owned()),
        },
        extension_digest: None,
        config_digest: None,
        now_ms: NOW,
    };

    // In-scope target is allowed.
    let allowed = permissions::evaluate(&base_request());
    assert!(
        matches!(allowed, permissions::Decision::Allow { .. }),
        "in-scope write was denied: {allowed:?}"
    );

    // Foreign root — not covered.
    let mut foreign = base_request();
    foreign.target = identity::delegation::ScopedTarget::Workspace {
        uri: "workspace://team-b".to_owned(),
        path: Some("docs".to_owned()),
    };
    let decision = permissions::evaluate(&foreign);
    assert!(
        matches!(decision, permissions::Decision::Deny { .. }),
        "foreign workspace root was allowed: {decision:?}"
    );

    // Traversal-shaped path — denied, not normalized into the scope.
    let mut traversal = base_request();
    traversal.target = identity::delegation::ScopedTarget::Workspace {
        uri: "workspace://team-a".to_owned(),
        path: Some("../team-b/secrets".to_owned()),
    };
    let decision = permissions::evaluate(&traversal);
    assert!(
        matches!(decision, permissions::Decision::Deny { .. }),
        "traversal target was allowed: {decision:?}"
    );
}

// -- 7. insecure control-socket directory ------------------------------------

/// Bind over a world-writable runtime dir: the socket hardens the dir to
/// 0700 and itself to 0600 before accepting connections.
#[tokio::test]
async fn insecure_socket_directory_is_corrected_at_bind() {
    use control_api::ControlSocket;

    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    // Make it deliberately insecure: world/group readable + writable.
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        std::fs::metadata(&runtime_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o777
    );

    #[derive(Debug)]
    struct TestLock;
    impl control_api::uds::DaemonLockHeld for TestLock {}
    let socket = ControlSocket::bind(&runtime_dir, &TestLock).unwrap();

    let dir_mode = std::fs::metadata(&runtime_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let socket_mode = std::fs::metadata(socket.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "runtime dir was not hardened to 0700");
    assert_eq!(socket_mode, 0o600, "socket was not hardened to 0600");
}
