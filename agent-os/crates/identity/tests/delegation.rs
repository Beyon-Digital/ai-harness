//! Delegation-chain derivation, integrity validation, and grant-backed persistence.
//!
//! The pure tests build chains in memory; the persistence tests run against a
//! real SQLite store opened in a temporary runtime directory.

use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;

use domain::ids::{
    ActorId, CapabilityGrantId, CommandId, DaemonInstanceId, DelegationChainId, PrincipalId,
};
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{
    Capability, CapabilityAction, CapabilityFamily, DelegationChain, GrantScope, Hop, ScopedTarget,
    derive_child_chain, encode_grant_scope, load_chain, load_grant_scopes, persist_hop,
    validate_chain,
};
use kernel_store::models::{NewCapabilityGrant, NewDelegationHop};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use proptest::prelude::*;
use testkit::ids::DeterministicIds;

const WORKSPACE_READ: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Read);
const WORKSPACE_WRITE: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Write);
const NETWORK_CONNECT: Capability =
    Capability::new(CapabilityFamily::Network, CapabilityAction::Connect);
const SECRET_USE: Capability = Capability::new(CapabilityFamily::Secret, CapabilityAction::Use);

const ALL_CAPABILITIES: [Capability; 20] = [
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Read),
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Write),
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Fork),
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Merge),
    Capability::new(
        CapabilityFamily::Workspace,
        CapabilityAction::TransferExclusive,
    ),
    Capability::new(
        CapabilityFamily::Workspace,
        CapabilityAction::SharedCoordinated,
    ),
    Capability::new(CapabilityFamily::Network, CapabilityAction::Connect),
    Capability::new(CapabilityFamily::Secret, CapabilityAction::Use),
    Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct),
    Capability::new(CapabilityFamily::Agent, CapabilityAction::Spawn),
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Install),
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Enable),
    Capability::new(CapabilityFamily::Extension, CapabilityAction::Disable),
    Capability::new(CapabilityFamily::Config, CapabilityAction::Propose),
    Capability::new(CapabilityFamily::Config, CapabilityAction::Test),
    Capability::new(CapabilityFamily::Config, CapabilityAction::Activate),
    Capability::new(CapabilityFamily::Config, CapabilityAction::Rollback),
    Capability::new(CapabilityFamily::Effect, CapabilityAction::Reconcile),
    Capability::new(CapabilityFamily::Effect, CapabilityAction::ResolveUnknown),
    Capability::new(CapabilityFamily::Resource, CapabilityAction::Reserve),
];

fn provider() -> DeterministicIds {
    DeterministicIds::new(1_700_000_000_000)
}

/// Builds a structurally valid hop whose grant ids back its capabilities.
fn hop(ids: &DeterministicIds, hop_index: u32, capabilities: &[Capability]) -> Hop {
    let mut capabilities = capabilities.to_vec();
    capabilities.sort();
    capabilities.dedup();
    let grant_ids = capabilities
        .iter()
        .map(|_| CapabilityGrantId::new(ids))
        .collect();
    Hop {
        hop_index,
        actor_id: ActorId::new(ids),
        run_id: None,
        grant_ids,
        capabilities,
    }
}

fn chain(ids: &DeterministicIds, hops: Vec<Hop>) -> DelegationChain {
    DelegationChain {
        chain_id: DelegationChainId::new(ids),
        hops,
    }
}

fn chain_of_hops(ids: &DeterministicIds, layers: &[Vec<Capability>]) -> DelegationChain {
    let hops = layers
        .iter()
        .enumerate()
        .map(|(index, capabilities)| hop(ids, u32::try_from(index).unwrap(), capabilities))
        .collect();
    chain(ids, hops)
}

fn context(ids: &DeterministicIds, daemon_epoch: u64) -> TxContext {
    TxContext {
        daemon_epoch,
        principal_id: PrincipalId::new(ids),
        command_id: CommandId::new(ids),
        correlation_id: None,
    }
}

async fn open_store(dir: &Path) -> (SqliteKernelStore, u64) {
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.join("kernel.db"),
        pool_max_connections: 8,
        busy_timeout_ms: 5_000,
    })
    .await
    .unwrap();
    let ids = provider();
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .unwrap();
    (store, fence.epoch.0)
}

async fn insert_grant(
    txn: &mut dyn KernelTxn,
    ids: &DeterministicIds,
    capability: Capability,
    delegated_from: Option<CapabilityGrantId>,
    created_at_ms: i64,
) -> CapabilityGrantId {
    let grant_id = CapabilityGrantId::new(ids);
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id,
            principal_id: PrincipalId::new(ids),
            actor_id: ActorId::new(ids),
            run_id: None,
            capability_id: capability.to_string(),
            scope: Vec::new(),
            delegated_from_grant_id: delegated_from,
            expires_at_ms: None,
            revoked_at_ms: None,
            created_at_ms,
        })
        .await
        .unwrap();
    grant_id
}

fn grant_id_blob(grant_ids: &[CapabilityGrantId]) -> Vec<u8> {
    grant_ids
        .iter()
        .flat_map(|id| id.to_string().into_bytes())
        .collect()
}

#[test]
fn capability_tokens_round_trip_the_contract() {
    assert_eq!(CapabilityFamily::Workspace.as_str(), "workspace");
    assert_eq!(CapabilityFamily::Resource.as_str(), "resource");
    assert_eq!(
        CapabilityAction::TransferExclusive.as_str(),
        "transfer_exclusive_write"
    );
    assert_eq!(
        CapabilityAction::SharedCoordinated.as_str(),
        "shared_coordinated_write"
    );
    assert_eq!(CapabilityAction::SignOrAct.as_str(), "sign_or_act");
    assert_eq!(CapabilityAction::ResolveUnknown.as_str(), "resolve_unknown");

    for capability in ALL_CAPABILITIES {
        assert_eq!(
            capability.to_string().parse::<Capability>().unwrap(),
            capability
        );
        assert!(capability.is_supported());
    }

    for rejected in [
        "",
        "workspace",
        "workspace.",
        ".read",
        "network.read",
        "workspace.frobnicate",
        "Workspace.Read",
    ] {
        assert!(
            rejected.parse::<Capability>().is_err(),
            "{rejected} must be rejected"
        );
    }
}

#[test]
fn derive_returns_the_requested_subset_sorted_and_deduplicated() {
    let ids = provider();
    let parent = chain_of_hops(
        &ids,
        &[vec![WORKSPACE_READ, WORKSPACE_WRITE, NETWORK_CONNECT]],
    );
    let derived =
        derive_child_chain(&parent, &[NETWORK_CONNECT, WORKSPACE_READ, NETWORK_CONNECT]).unwrap();
    assert_eq!(derived, vec![WORKSPACE_READ, NETWORK_CONNECT]);
}

#[test]
fn derive_of_an_empty_request_succeeds_with_an_empty_set() {
    let ids = provider();
    let parent = chain_of_hops(&ids, &[vec![WORKSPACE_READ]]);
    assert!(derive_child_chain(&parent, &[]).unwrap().is_empty());
}

#[test]
fn derive_rejects_a_request_the_parent_does_not_hold() {
    let ids = provider();
    let parent = chain_of_hops(&ids, &[vec![WORKSPACE_READ]]);
    let error = derive_child_chain(&parent, &[SECRET_USE]).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[test]
fn derive_rejects_a_parent_chain_that_is_not_grant_backed() {
    let ids = provider();
    let mut root = hop(&ids, 0, &[WORKSPACE_READ]);
    root.grant_ids.clear();
    let parent = chain(&ids, vec![root]);
    let error = derive_child_chain(&parent, &[WORKSPACE_READ]).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[test]
fn validate_accepts_a_contiguous_subset_chain() {
    let ids = provider();
    let valid = chain_of_hops(
        &ids,
        &[
            vec![WORKSPACE_READ, WORKSPACE_WRITE],
            vec![WORKSPACE_WRITE],
            vec![],
        ],
    );
    validate_chain(&valid).unwrap();
}

#[test]
fn validate_accepts_an_empty_chain_without_inferring_authority() {
    let ids = provider();
    validate_chain(&chain(&ids, vec![])).unwrap();
}

#[test]
fn validate_rejects_capabilities_without_grant_ids() {
    let ids = provider();
    let mut root = hop(&ids, 0, &[WORKSPACE_READ]);
    root.grant_ids.clear();
    let error = validate_chain(&chain(&ids, vec![root])).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[test]
fn validate_rejects_more_capabilities_than_grants() {
    let ids = provider();
    let mut root = hop(&ids, 0, &[WORKSPACE_READ, WORKSPACE_WRITE]);
    root.grant_ids.truncate(1);
    let error = validate_chain(&chain(&ids, vec![root])).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[test]
fn validate_rejects_a_tampered_gap_between_hops() {
    let ids = provider();
    let first = hop(&ids, 0, &[WORKSPACE_READ]);
    let second = hop(&ids, 2, &[WORKSPACE_READ]);
    let error = validate_chain(&chain(&ids, vec![first, second])).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[test]
fn validate_rejects_a_duplicated_hop_index() {
    let ids = provider();
    let first = hop(&ids, 0, &[WORKSPACE_READ]);
    let second = hop(&ids, 0, &[WORKSPACE_READ]);
    let error = validate_chain(&chain(&ids, vec![first, second])).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[test]
fn validate_rejects_a_duplicated_capability() {
    let ids = provider();
    let mut root = hop(&ids, 0, &[WORKSPACE_READ, WORKSPACE_WRITE]);
    root.capabilities = vec![WORKSPACE_READ, WORKSPACE_READ];
    let error = validate_chain(&chain(&ids, vec![root])).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[test]
fn validate_rejects_capabilities_that_exceed_the_parent() {
    let ids = provider();
    let invalid = chain_of_hops(
        &ids,
        &[vec![WORKSPACE_READ], vec![WORKSPACE_READ, WORKSPACE_WRITE]],
    );
    let error = validate_chain(&invalid).unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn persist_and_load_round_trip_a_grant_backed_chain() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let actor = ActorId::new(&ids);

    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();
    let read_grant = insert_grant(txn.as_mut(), &ids, WORKSPACE_READ, None, 10).await;
    let write_grant = insert_grant(txn.as_mut(), &ids, WORKSPACE_WRITE, None, 10).await;
    persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 0,
            actor_id: actor,
            run_id: None,
            grant_ids: vec![read_grant, write_grant],
            capabilities: vec![WORKSPACE_READ, WORKSPACE_WRITE],
        },
    )
    .await
    .unwrap();

    let parent = load_chain(txn.as_mut(), chain_id).await.unwrap();
    let derived = derive_child_chain(&parent, &[WORKSPACE_WRITE]).unwrap();
    assert_eq!(derived, vec![WORKSPACE_WRITE]);

    let child_grant =
        insert_grant(txn.as_mut(), &ids, WORKSPACE_WRITE, Some(write_grant), 11).await;
    persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 1,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![child_grant],
            capabilities: derived.clone(),
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let mut verify = store.begin_write(context(&ids, epoch)).await.unwrap();
    let loaded = load_chain(verify.as_mut(), chain_id).await.unwrap();
    assert_eq!(loaded.chain_id, chain_id);
    assert_eq!(loaded.hops.len(), 2);
    assert_eq!(loaded.hops[0].hop_index, 0);
    assert_eq!(loaded.hops[0].actor_id, actor);
    assert_eq!(loaded.hops[0].grant_ids, vec![read_grant, write_grant]);
    assert_eq!(
        loaded.hops[0].capabilities,
        vec![WORKSPACE_READ, WORKSPACE_WRITE]
    );
    assert_eq!(loaded.hops[1].hop_index, 1);
    assert_eq!(loaded.hops[1].grant_ids, vec![child_grant]);
    assert_eq!(loaded.hops[1].capabilities, vec![WORKSPACE_WRITE]);
    verify.rollback().await.unwrap();
}

#[tokio::test]
async fn load_grant_scopes_round_trips_persisted_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    let workspace_scope = ScopedTarget::Workspace {
        uri: "workspace://acme/proj".to_owned(),
        path: Some("src/lib.rs".to_owned()),
    };
    let scoped_grant_id = CapabilityGrantId::new(&ids);
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: scoped_grant_id,
            principal_id: PrincipalId::new(&ids),
            actor_id: ActorId::new(&ids),
            run_id: None,
            capability_id: WORKSPACE_READ.to_string(),
            scope: encode_grant_scope(Some(&workspace_scope)),
            delegated_from_grant_id: None,
            expires_at_ms: Some(1_700_000_000_000),
            revoked_at_ms: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();

    let unscoped_grant_id = CapabilityGrantId::new(&ids);
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: unscoped_grant_id,
            principal_id: PrincipalId::new(&ids),
            actor_id: ActorId::new(&ids),
            run_id: None,
            capability_id: WORKSPACE_WRITE.to_string(),
            scope: encode_grant_scope(None),
            delegated_from_grant_id: None,
            expires_at_ms: None,
            revoked_at_ms: Some(1_600_000_000_000),
            created_at_ms: 10,
        })
        .await
        .unwrap();

    let loaded = load_grant_scopes(txn.as_mut(), &[scoped_grant_id, unscoped_grant_id])
        .await
        .unwrap();
    assert_eq!(
        loaded,
        vec![
            GrantScope {
                grant_id: scoped_grant_id,
                capability: WORKSPACE_READ,
                scope: Some(workspace_scope.clone()),
                expires_at_ms: Some(1_700_000_000_000),
            },
            GrantScope {
                grant_id: unscoped_grant_id,
                capability: WORKSPACE_WRITE,
                scope: None,
                expires_at_ms: Some(1_600_000_000_000),
            },
        ]
    );
    txn.commit().await.unwrap();

    let mut verify = store.begin_write(context(&ids, epoch)).await.unwrap();
    let reloaded = load_grant_scopes(verify.as_mut(), &[scoped_grant_id])
        .await
        .unwrap();
    assert_eq!(
        reloaded,
        vec![GrantScope {
            grant_id: scoped_grant_id,
            capability: WORKSPACE_READ,
            scope: Some(workspace_scope),
            expires_at_ms: Some(1_700_000_000_000),
        }]
    );
    verify.rollback().await.unwrap();
}

#[tokio::test]
async fn load_grant_scopes_fails_closed_on_malformed_blobs_and_missing_grants() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    let malformed_grant = CapabilityGrantId::new(&ids);
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: malformed_grant,
            principal_id: PrincipalId::new(&ids),
            actor_id: ActorId::new(&ids),
            run_id: None,
            capability_id: WORKSPACE_READ.to_string(),
            scope: b"agentd-grant-scope-v1\tmystery\n".to_vec(),
            delegated_from_grant_id: None,
            expires_at_ms: None,
            revoked_at_ms: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();
    let error = load_grant_scopes(txn.as_mut(), &[malformed_grant])
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let missing_grant = CapabilityGrantId::new(&ids);
    let error = load_grant_scopes(txn.as_mut(), &[missing_grant])
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn persist_rejects_a_grant_that_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    let missing = CapabilityGrantId::new(&ids);
    let error = persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 0,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![missing],
            capabilities: vec![WORKSPACE_READ],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn persist_rejects_capabilities_its_grants_do_not_carry() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();
    let read_grant = insert_grant(txn.as_mut(), &ids, WORKSPACE_READ, None, 10).await;

    let error = persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 0,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![read_grant],
            capabilities: vec![WORKSPACE_WRITE],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn persist_rejects_out_of_order_and_duplicate_hops() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();
    let empty = || Hop {
        hop_index: 0,
        actor_id: ActorId::new(&ids),
        run_id: None,
        grant_ids: Vec::new(),
        capabilities: Vec::new(),
    };

    persist_hop(txn.as_mut(), chain_id, empty()).await.unwrap();
    let duplicate = persist_hop(txn.as_mut(), chain_id, empty())
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::FailedPrecondition);

    let mut skipped = empty();
    skipped.hop_index = 5;
    let gap = persist_hop(txn.as_mut(), chain_id, skipped)
        .await
        .unwrap_err();
    assert_eq!(gap.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn persist_rejects_a_child_hop_that_exceeds_its_parent() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();
    let read_grant = insert_grant(txn.as_mut(), &ids, WORKSPACE_READ, None, 10).await;
    let write_grant = insert_grant(txn.as_mut(), &ids, WORKSPACE_WRITE, None, 10).await;

    persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 0,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![read_grant],
            capabilities: vec![WORKSPACE_READ],
        },
    )
    .await
    .unwrap();

    let error = persist_hop(
        txn.as_mut(),
        chain_id,
        Hop {
            hop_index: 1,
            actor_id: ActorId::new(&ids),
            run_id: None,
            grant_ids: vec![write_grant],
            capabilities: vec![WORKSPACE_WRITE],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn the_store_rejects_a_duplicate_hop_row() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    for _ in 0..2 {
        let result = txn
            .security()
            .insert_delegation_hop(NewDelegationHop {
                chain_id,
                hop_index: 0,
                principal_or_actor_id: ActorId::new(&ids).to_string(),
                run_id: None,
                capability_grant_ids: Vec::new(),
            })
            .await;
        if let Err(error) = result {
            assert_eq!(error.code(), ErrorCode::Conflict);
            return;
        }
    }
    panic!("a duplicate delegation hop must be rejected");
}

#[tokio::test]
async fn load_rejects_a_hop_referencing_a_missing_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    let missing = CapabilityGrantId::new(&ids);
    txn.security()
        .insert_delegation_hop(NewDelegationHop {
            chain_id,
            hop_index: 0,
            principal_or_actor_id: ActorId::new(&ids).to_string(),
            run_id: None,
            capability_grant_ids: grant_id_blob(&[missing]),
        })
        .await
        .unwrap();

    let error = load_chain(txn.as_mut(), chain_id).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test]
async fn load_rejects_a_chain_with_a_link_gap() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    txn.security()
        .insert_delegation_hop(NewDelegationHop {
            chain_id,
            hop_index: 1,
            principal_or_actor_id: ActorId::new(&ids).to_string(),
            run_id: None,
            capability_grant_ids: Vec::new(),
        })
        .await
        .unwrap();

    let error = load_chain(txn.as_mut(), chain_id).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn load_rejects_an_unrecognized_grant_capability() {
    let dir = tempfile::tempdir().unwrap();
    let (store, epoch) = open_store(dir.path()).await;
    let ids = provider();
    let chain_id = DelegationChainId::new(&ids);
    let mut txn = store.begin_write(context(&ids, epoch)).await.unwrap();

    let grant = CapabilityGrantId::new(&ids);
    txn.security()
        .insert_grant(NewCapabilityGrant {
            grant_id: grant,
            principal_id: PrincipalId::new(&ids),
            actor_id: ActorId::new(&ids),
            run_id: None,
            capability_id: "filesystem.write".to_owned(),
            scope: Vec::new(),
            delegated_from_grant_id: None,
            expires_at_ms: None,
            revoked_at_ms: None,
            created_at_ms: 10,
        })
        .await
        .unwrap();
    txn.security()
        .insert_delegation_hop(NewDelegationHop {
            chain_id,
            hop_index: 0,
            principal_or_actor_id: ActorId::new(&ids).to_string(),
            run_id: None,
            capability_grant_ids: grant_id_blob(&[grant]),
        })
        .await
        .unwrap();

    let error = load_chain(txn.as_mut(), chain_id).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn derived_capability_sets_are_always_subsets_of_the_parent(
        parent_picks in prop::collection::vec(0usize..ALL_CAPABILITIES.len(), 0..8),
        request_picks in prop::collection::vec(0usize..ALL_CAPABILITIES.len(), 0..8),
        extra_hops in 0usize..3,
    ) {
        let ids = provider();
        let parent_capabilities: Vec<Capability> = parent_picks
            .iter()
            .map(|index| ALL_CAPABILITIES[*index])
            .collect();
        let mut layers = vec![parent_capabilities];
        for _ in 0..extra_hops {
            let Some(previous) = layers.last() else {
                break;
            };
            let next: Vec<Capability> = previous.iter().step_by(2).copied().collect();
            layers.push(next);
        }
        let parent = chain_of_hops(&ids, &layers);
        let requested: Vec<Capability> = request_picks
            .iter()
            .map(|index| ALL_CAPABILITIES[*index])
            .collect();
        let tip: BTreeSet<Capability> = layers
            .last()
            .map(|layer| layer.iter().copied().collect())
            .unwrap_or_default();

        match derive_child_chain(&parent, &requested) {
            Ok(derived) => {
                for capability in &derived {
                    prop_assert!(tip.contains(capability));
                }
                let expected: Vec<Capability> = requested
                    .iter()
                    .copied()
                    .filter(|capability| tip.contains(capability))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                prop_assert_eq!(derived, expected);
            }
            Err(error) => {
                prop_assert_eq!(error.code(), ErrorCode::InvalidArgument);
                prop_assert!(requested.iter().any(|capability| !tip.contains(capability)));
            }
        }
    }
}

#[test]
fn from_str_is_used_for_canonical_grant_capabilities() {
    assert_eq!(
        Capability::from_str("workspace.read").unwrap(),
        WORKSPACE_READ
    );
}
