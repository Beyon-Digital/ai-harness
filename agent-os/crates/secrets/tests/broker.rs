//! Broker acceptance tests: redaction, authorization-before-access, scope
//! narrowing, joint-egress approval, sign-or-act preference, the in-memory
//! backend contract, and audit content (R4.1-R4.6, P4, N1).
//!
//! Every case drives `SecretsBroker` over a counting in-memory backend, so a
//! test can assert that a denied or approval-bound request never reaches the
//! store at all. Audit records are captured through a recording sink and must
//! carry actor, run, uri, and outcome only.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::ids::{ActorId, CapabilityGrantId, DelegationChainId, PrincipalId, RunId};
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily, DelegationChain, Hop};
use permissions::{GrantScope, ScopedTarget};
use secrets::broker::InMemorySecretStore;
use secrets::{
    ActResult, AuditSink, SecretAuditRecord, SecretMetadata, SecretStore, SecretUseOutcome,
    SecretUseRequest, SecretValue, SecretsBroker,
};
use testkit::clock::TestClock;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const SECRET_URI: &str = "secret://prod/api-key";
const SECRET_TEXT: &str = "super-secret-material-xyz";
const SECRET_BYTES: &[u8] = SECRET_TEXT.as_bytes();

const SECRET_USE: Capability = Capability::new(CapabilityFamily::Secret, CapabilityAction::Use);
const SECRET_SIGN_OR_ACT: Capability =
    Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct);
const NETWORK_CONNECT: Capability =
    Capability::new(CapabilityFamily::Network, CapabilityAction::Connect);

/// Deterministic identifiers shared by one test case.
struct Fixtures {
    ids: DeterministicIds,
    principal: PrincipalId,
    actor: ActorId,
    run: RunId,
    chain_id: DelegationChainId,
}

impl Fixtures {
    fn new() -> Self {
        let ids = DeterministicIds::new(SEED_MS);
        Self {
            principal: PrincipalId::new(&ids),
            actor: ActorId::new(&ids),
            run: RunId::new(&ids),
            chain_id: DelegationChainId::new(&ids),
            ids,
        }
    }

    fn grant_ids(&self, count: usize) -> Vec<CapabilityGrantId> {
        (0..count)
            .map(|_| CapabilityGrantId::new(&self.ids))
            .collect()
    }
}

/// Builds a one-hop chain plus matching grant rows for `entries`.
fn held(
    fixtures: &Fixtures,
    entries: &[(Capability, Option<ScopedTarget>)],
) -> (DelegationChain, Vec<GrantScope>) {
    let grant_ids = fixtures.grant_ids(entries.len());
    let capabilities = entries.iter().map(|(capability, _)| *capability).collect();
    let grant_scopes = entries
        .iter()
        .zip(&grant_ids)
        .map(|((capability, scope), grant_id)| GrantScope {
            grant_id: *grant_id,
            capability: *capability,
            scope: scope.clone(),
            expires_at_ms: None,
        })
        .collect();
    let hop = Hop {
        hop_index: 0,
        actor_id: fixtures.actor,
        run_id: Some(fixtures.run),
        grant_ids,
        capabilities,
    };
    (
        DelegationChain {
            chain_id: fixtures.chain_id,
            hops: vec![hop],
        },
        grant_scopes,
    )
}

fn request(
    fixtures: &Fixtures,
    chain: DelegationChain,
    grant_scopes: Vec<GrantScope>,
    uri: &str,
    egress: Option<Vec<String>>,
    operation: &str,
) -> SecretUseRequest {
    SecretUseRequest {
        principal_id: fixtures.principal,
        actor_id: fixtures.actor,
        run_id: Some(fixtures.run),
        chain,
        grant_scopes,
        uri: uri.to_string(),
        egress,
        operation: operation.to_string(),
    }
}

fn metadata(uri: &str) -> SecretMetadata {
    SecretMetadata {
        uri: uri.to_string(),
        kind: "api-key".to_string(),
        scopes: vec!["read".to_string()],
    }
}

/// In-memory backend wrapped with call counters for authorization-order
/// assertions.
struct CountingStore {
    inner: InMemorySecretStore,
    metadata_calls: AtomicUsize,
    get_calls: AtomicUsize,
    act_calls: AtomicUsize,
}

impl CountingStore {
    fn seeded(metadata: SecretMetadata, value: &[u8]) -> Self {
        let inner = InMemorySecretStore::new();
        inner.insert(metadata, value);
        Self {
            inner,
            metadata_calls: AtomicUsize::new(0),
            get_calls: AtomicUsize::new(0),
            act_calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> (usize, usize, usize) {
        (
            self.metadata_calls.load(Ordering::SeqCst),
            self.get_calls.load(Ordering::SeqCst),
            self.act_calls.load(Ordering::SeqCst),
        )
    }
}

#[async_trait]
impl SecretStore for CountingStore {
    async fn metadata(&self, uri: &str) -> errors::Result<SecretMetadata> {
        self.metadata_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.metadata(uri).await
    }

    async fn get(&self, uri: &str) -> errors::Result<SecretValue> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.get(uri).await
    }

    async fn sign_or_act(
        &self,
        uri: &str,
        action: &str,
        payload: &[u8],
    ) -> errors::Result<ActResult> {
        self.act_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.sign_or_act(uri, action, payload).await
    }
}

#[derive(Default)]
struct RecordingSink {
    records: Mutex<Vec<SecretAuditRecord>>,
}

impl RecordingSink {
    fn records(&self) -> Vec<SecretAuditRecord> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl AuditSink for RecordingSink {
    fn record(&self, record: &SecretAuditRecord) {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(record.clone());
    }
}

fn broker_with(store: Arc<CountingStore>) -> (SecretsBroker, Arc<RecordingSink>) {
    let audit = Arc::new(RecordingSink::default());
    let broker = SecretsBroker::new(store, Arc::new(TestClock::new(SEED_MS)), audit.clone());
    (broker, audit)
}

#[tokio::test]
async fn debug_and_error_paths_never_render_secret_material() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, _) = broker_with(store.clone());

    let value = store.inner.get(SECRET_URI).await.expect("seeded value");
    assert_eq!(format!("{value:?}"), "[REDACTED]");

    let (chain, grants) = held(&fixtures, &[]);
    let denied = request(&fixtures, chain, grants, SECRET_URI, None, "read");
    let error = broker.use_secret(&denied).await.expect_err("denied");
    assert!(!error.to_string().contains(SECRET_TEXT));
    assert!(!format!("{error:?}").contains(SECRET_TEXT));

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None)]);
    let missing = request(
        &fixtures,
        chain,
        grants,
        "secret://prod/absent",
        None,
        "read",
    );
    let error = broker.use_secret(&missing).await.expect_err("missing");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert!(!error.to_string().contains(SECRET_TEXT));
    assert!(!format!("{error:?}").contains(SECRET_TEXT));
}

#[tokio::test]
async fn unauthorized_use_is_denied_before_the_store_is_touched() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, audit) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(NETWORK_CONNECT, None)]);
    let denied = request(&fixtures, chain, grants, SECRET_URI, None, "read");
    let error = broker.use_secret(&denied).await.expect_err("denied");

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert_eq!(store.calls(), (0, 0, 0));
    let records = audit.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, SecretUseOutcome::Denied);
    assert!(!format!("{records:?}").contains(SECRET_TEXT));
}

#[tokio::test]
async fn joint_egress_requires_approval_and_releases_no_material() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, audit) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None), (NETWORK_CONNECT, None)]);
    let use_request = request(
        &fixtures,
        chain,
        grants,
        SECRET_URI,
        Some(vec!["*".to_string()]),
        "read",
    );
    let error = broker
        .use_secret(&use_request)
        .await
        .expect_err("approval required");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert_eq!(store.calls(), (0, 0, 0));
    let records = audit.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, SecretUseOutcome::ApprovalRequired);
    assert!(!format!("{records:?}").contains(SECRET_TEXT));
}

#[tokio::test]
async fn unbounded_egress_without_network_authority_requires_approval() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, _) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None)]);
    let use_request = request(
        &fixtures,
        chain,
        grants,
        SECRET_URI,
        Some(Vec::new()),
        "read",
    );
    let error = broker
        .use_secret(&use_request)
        .await
        .expect_err("approval required");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(store.calls(), (0, 0, 0));
}

#[tokio::test]
async fn scoped_grant_covers_child_secret_uris_without_widening() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, _) = broker_with(store.clone());

    let prefix = Some(ScopedTarget::Secret {
        uri: "secret://prod".to_string(),
        egress: None,
    });
    let (chain, grants) = held(&fixtures, &[(SECRET_USE, prefix)]);
    let allowed = request(&fixtures, chain, grants, SECRET_URI, None, "read");
    let value = broker.use_secret(&allowed).await.expect("authorized");
    assert_eq!(value.expose(), SECRET_BYTES);

    let foreign = Some(ScopedTarget::Secret {
        uri: "secret://staging".to_string(),
        egress: None,
    });
    let (chain, grants) = held(&fixtures, &[(SECRET_USE, foreign)]);
    let denied = request(&fixtures, chain, grants, SECRET_URI, None, "read");
    let error = broker.use_secret(&denied).await.expect_err("out of scope");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(store.calls(), (0, 1, 0));
}

#[tokio::test]
async fn bounded_egress_below_the_joint_rule_is_allowed() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, _) = broker_with(store.clone());

    let scope = Some(ScopedTarget::Secret {
        uri: "secret://prod".to_string(),
        egress: Some(vec!["api.example.com".to_string()]),
    });
    let (chain, grants) = held(&fixtures, &[(SECRET_USE, scope)]);
    let allowed = request(
        &fixtures,
        chain,
        grants,
        SECRET_URI,
        Some(vec!["api.example.com".to_string()]),
        "read",
    );
    let value = broker.use_secret(&allowed).await.expect("authorized");
    assert_eq!(value.expose(), SECRET_BYTES);
    assert_eq!(store.calls(), (0, 1, 0));
}

#[tokio::test]
async fn sign_or_act_is_preferred_over_raw_material() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, audit) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None), (SECRET_SIGN_OR_ACT, None)]);
    let act_request = request(&fixtures, chain, grants, SECRET_URI, None, "sign");
    let result = broker
        .sign_or_act(&act_request, b"payload")
        .await
        .expect("authorized");

    assert!(!result.reference.is_empty());
    assert!(!result.reference.contains(SECRET_TEXT));
    assert_eq!(store.calls(), (0, 0, 1));
    let records = audit.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, SecretUseOutcome::Acted);
}

#[tokio::test]
async fn sign_or_act_capability_does_not_grant_raw_access() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, _) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_SIGN_OR_ACT, None)]);
    let raw = request(
        &fixtures,
        chain.clone(),
        grants.clone(),
        SECRET_URI,
        None,
        "read",
    );
    let error = broker.use_secret(&raw).await.expect_err("denied");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(store.calls(), (0, 0, 0));

    let act_request = request(&fixtures, chain, grants, SECRET_URI, None, "sign");
    let result = broker
        .sign_or_act(&act_request, b"payload")
        .await
        .expect("authorized");
    assert!(!result.reference.is_empty());
    assert_eq!(store.calls(), (0, 0, 1));
}

#[tokio::test]
async fn in_memory_store_implements_the_shared_contract() {
    let store = InMemorySecretStore::new();
    store.insert(metadata(SECRET_URI), SECRET_BYTES);

    let loaded = store.metadata(SECRET_URI).await.expect("metadata");
    assert_eq!(loaded, metadata(SECRET_URI));

    let value = store.get(SECRET_URI).await.expect("value");
    assert_eq!(value.expose(), SECRET_BYTES);
    assert_eq!(format!("{value:?}"), "[REDACTED]");

    let missing = store
        .get("secret://prod/absent")
        .await
        .expect_err("missing value");
    assert_eq!(missing.code(), ErrorCode::NotFound);
    let missing_metadata = store
        .metadata("secret://prod/absent")
        .await
        .expect_err("missing metadata");
    assert_eq!(missing_metadata.code(), ErrorCode::NotFound);

    let first = store
        .sign_or_act(SECRET_URI, "sign", b"payload")
        .await
        .expect("act");
    let again = store
        .sign_or_act(SECRET_URI, "sign", b"payload")
        .await
        .expect("act");
    let other = store
        .sign_or_act(SECRET_URI, "decrypt", b"payload")
        .await
        .expect("act");
    assert_eq!(first, again);
    assert_ne!(first, other);
    assert!(!first.reference.contains(SECRET_TEXT));
}

#[tokio::test]
async fn audit_records_carry_identity_uri_and_outcome_only() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, audit) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None)]);
    let allowed = request(&fixtures, chain, grants, SECRET_URI, None, "read");
    broker.use_secret(&allowed).await.expect("authorized");

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None)]);
    let missing = request(
        &fixtures,
        chain,
        grants,
        "secret://prod/absent",
        None,
        "read",
    );
    let error = broker.use_secret(&missing).await.expect_err("missing");
    assert_eq!(error.code(), ErrorCode::NotFound);

    let records = audit.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].actor_id, fixtures.actor);
    assert_eq!(records[0].run_id, Some(fixtures.run));
    assert_eq!(records[0].uri, SECRET_URI);
    assert_eq!(records[0].outcome, SecretUseOutcome::Allowed);
    assert_eq!(records[1].outcome, SecretUseOutcome::Failed);
    let rendered = format!("{records:?}");
    assert!(rendered.contains(SECRET_URI));
    assert!(!rendered.contains(SECRET_TEXT));
}

#[tokio::test]
async fn metadata_resolution_goes_through_authorization() {
    let fixtures = Fixtures::new();
    let store = Arc::new(CountingStore::seeded(metadata(SECRET_URI), SECRET_BYTES));
    let (broker, audit) = broker_with(store.clone());

    let (chain, grants) = held(&fixtures, &[(SECRET_SIGN_OR_ACT, None)]);
    let denied = request(&fixtures, chain, grants, SECRET_URI, None, "metadata");
    let error = broker.resolve_metadata(&denied).await.expect_err("denied");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(store.calls(), (0, 0, 0));

    let (chain, grants) = held(&fixtures, &[(SECRET_USE, None)]);
    let allowed = request(&fixtures, chain, grants, SECRET_URI, None, "metadata");
    let loaded = broker.resolve_metadata(&allowed).await.expect("authorized");
    assert_eq!(loaded, metadata(SECRET_URI));
    assert_eq!(store.calls(), (1, 0, 0));
    let records = audit.records();
    assert_eq!(records[0].outcome, SecretUseOutcome::Denied);
    assert_eq!(records[1].outcome, SecretUseOutcome::Allowed);
}
