//! Opt-in macOS Keychain integration tests (R4.6, R4.7).
//!
//! These tests run only when `AGENTD_KEYCHAIN_TEST=1`; otherwise each one
//! prints a note and returns, so CI stays deterministic and the in-memory
//! backend remains the documented contract carrier. When enabled they use a
//! dedicated service name and create and delete their own items, even on
//! failure.

#![cfg(target_os = "macos")]

use std::sync::Arc;

use domain::ids::{ActorId, CapabilityGrantId, DelegationChainId, PrincipalId, RunId};
use errors::codes::ErrorCode;
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily, DelegationChain, Hop};
use permissions::GrantScope;
use secrets::{
    MacOsKeychainSecretStore, NullAuditSink, SecretMetadata, SecretStore, SecretUseRequest,
    SecretsBroker,
};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const SERVICE: &str = "dev.agentd.secrets.sec004.opt-in";
const SECRET_USE: Capability = Capability::new(CapabilityFamily::Secret, CapabilityAction::Use);

/// Reports whether the opt-in gate is enabled, noting the skip otherwise.
fn keychain_enabled() -> bool {
    if std::env::var("AGENTD_KEYCHAIN_TEST").as_deref() == Ok("1") {
        true
    } else {
        println!(
            "AGENTD_KEYCHAIN_TEST is not set to 1; skipping the opt-in Keychain integration test"
        );
        false
    }
}

/// Deletes the test's own item when dropped, even on assertion failure.
struct ItemGuard {
    store: MacOsKeychainSecretStore,
    uri: String,
}

impl Drop for ItemGuard {
    fn drop(&mut self) {
        let _ = self.store.delete(&self.uri);
    }
}

fn metadata(uri: &str) -> SecretMetadata {
    SecretMetadata {
        uri: uri.to_string(),
        kind: "api-key".to_string(),
        scopes: vec!["read".to_string()],
    }
}

#[tokio::test]
async fn keychain_round_trip_matches_the_shared_contract() {
    if !keychain_enabled() {
        return;
    }
    let store = MacOsKeychainSecretStore::new(SERVICE);
    let uri = "secret://agentd-sec004-contract/api-key";
    store.delete(uri).expect("pre-test cleanup");
    let guard = ItemGuard {
        store: store.clone(),
        uri: uri.to_string(),
    };
    let value = b"keychain-contract-value";
    store.put(&metadata(uri), value).expect("item is created");

    let loaded = store.get(uri).await.expect("authorized read");
    assert_eq!(loaded.expose(), value);
    assert_eq!(format!("{loaded:?}"), "[REDACTED]");

    let loaded_metadata = store.metadata(uri).await.expect("metadata");
    assert_eq!(loaded_metadata, metadata(uri));

    let unsupported = store
        .sign_or_act(uri, "sign", b"payload")
        .await
        .expect_err("sign-or-act is not supported by the Keychain backend");
    assert_eq!(unsupported.code(), ErrorCode::FailedPrecondition);

    guard.store.delete(uri).expect("item is deleted");
    let missing = store.get(uri).await.expect_err("deleted item");
    assert_eq!(missing.code(), ErrorCode::NotFound);
    let missing_metadata = store.metadata(uri).await.expect_err("deleted item");
    assert_eq!(missing_metadata.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn broker_reads_authorized_material_from_the_keychain() {
    if !keychain_enabled() {
        return;
    }
    let store = MacOsKeychainSecretStore::new(SERVICE);
    let uri = "secret://agentd-sec004-broker/token";
    store.delete(uri).expect("pre-test cleanup");
    let _guard = ItemGuard {
        store: store.clone(),
        uri: uri.to_string(),
    };
    let value = b"broker-keychain-value";
    store.put(&metadata(uri), value).expect("item is created");

    let ids = DeterministicIds::new(SEED_MS);
    let principal = PrincipalId::new(&ids);
    let actor = ActorId::new(&ids);
    let run = RunId::new(&ids);
    let chain_id = DelegationChainId::new(&ids);
    let grant_id = CapabilityGrantId::new(&ids);
    let chain = DelegationChain {
        chain_id,
        hops: vec![Hop {
            hop_index: 0,
            actor_id: actor,
            run_id: Some(run),
            grant_ids: vec![grant_id],
            capabilities: vec![SECRET_USE],
        }],
    };
    let grants = vec![GrantScope {
        grant_id,
        capability: SECRET_USE,
        scope: None,
        expires_at_ms: None,
    }];
    let broker = SecretsBroker::new(
        Arc::new(store),
        Arc::new(TestClock::new(SEED_MS)),
        Arc::new(NullAuditSink),
    );
    let allowed = SecretUseRequest {
        principal_id: principal,
        actor_id: actor,
        run_id: Some(run),
        chain,
        grant_scopes: grants,
        uri: uri.to_string(),
        egress: None,
        operation: "read".to_string(),
    };
    let loaded = broker.use_secret(&allowed).await.expect("authorized");
    assert_eq!(loaded.expose(), value);

    let denied = SecretUseRequest {
        chain: DelegationChain {
            chain_id,
            hops: vec![Hop {
                hop_index: 0,
                actor_id: actor,
                run_id: Some(run),
                grant_ids: Vec::new(),
                capabilities: Vec::new(),
            }],
        },
        grant_scopes: Vec::new(),
        ..allowed
    };
    let error = broker.use_secret(&denied).await.expect_err("denied");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert!(!error.to_string().contains("broker-keychain-value"));
}
