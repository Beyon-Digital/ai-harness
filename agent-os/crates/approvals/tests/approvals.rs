//! Approval acceptance tests over a real SQLite kernel store and command
//! coordinator: golden digest framing, digest mismatch, expiry, double
//! response, wrong responder, digest invalidation, and `is_satisfied`
//! outcomes (R3.1-R3.6, P3, N1).

use std::str::FromStr;
use std::sync::Arc;

use approvals::{
    ApprovalDecision, ApprovalDeps, ApprovalDigest, ApprovalOutcome, ApprovalResolvedPayload,
    CMD_CREATE_APPROVAL_REQUEST, CMD_RESPOND_APPROVAL, DigestInput, Responder, canonical_digest,
    is_satisfied, register_handlers,
};
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandOutcome, CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use domain::generated::contract;
use domain::ids::{
    ActorId, ApprovalRequestId, CommandId, DaemonInstanceId, DeviceId, IdempotencyKey, PrincipalId,
    RunId,
};
use domain::security::{RetentionClass, SensitivityClass};
use domain::time::Clock;
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily};
use kernel_store::models::{ApprovalRequestRow, ApprovalResponseRow, OutboxEventRow};
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";
const TARGET: &str = "secret://prod/api-key|egress=*";
const GOLDEN_DIGEST: &str = "c81be7b01d7be688821a379513a67c5497b86fdf5b15466543a58651d4dc0b7b";

struct Fixture {
    principal: PrincipalId,
    other_principal: PrincipalId,
    actor: ActorId,
    run: RunId,
    device: DeviceId,
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    coordinator: CommandCoordinator,
    clock: Arc<TestClock>,
    ids: Arc<DeterministicIds>,
    fixture: Fixture,
    fence_epoch: u64,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        SqliteKernelStore::open(StoreConfig {
            path: dir.path().join(DB_FILE),
            pool_max_connections: 5,
            busy_timeout_ms: 5_000,
        })
        .await
        .expect("store opens"),
    );
    let ids = Arc::new(DeterministicIds::new(SEED_MS));
    let fixture = Fixture {
        principal: PrincipalId::new(ids.as_ref()),
        other_principal: PrincipalId::new(ids.as_ref()),
        actor: ActorId::new(ids.as_ref()),
        run: RunId::new(ids.as_ref()),
        device: DeviceId::new(ids.as_ref()),
    };
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(ids.as_ref()))
        .await
        .expect("fence acquired");
    let clock = Arc::new(TestClock::new(SEED_MS));
    let mut registry = CommandRegistry::new();
    register_handlers(
        &mut registry,
        ApprovalDeps {
            clock: clock.clone(),
        },
    )
    .expect("handlers register");
    let coordinator = CommandCoordinator::new(
        store.clone(),
        Arc::new(registry),
        Arc::new(FixedFence(fence.epoch.0)),
        clock.clone(),
        Arc::new(ArmedFaults::new()),
    );
    Harness {
        _dir: dir,
        store,
        coordinator,
        clock,
        ids,
        fixture,
        fence_epoch: fence.epoch.0,
    }
}

fn request_digest(marker: u8) -> RequestDigest {
    RequestDigest::from_str(&format!("{marker:02x}").repeat(32)).expect("valid request digest")
}

fn persisted_digest_from(text: &str) -> ApprovalDigest {
    ApprovalDigest::from_str(text).expect("persisted digest parses")
}

fn responder(harness: &Harness) -> Responder {
    Responder {
        principal_id: harness.fixture.principal,
        device_id: Some(harness.fixture.device),
    }
}

fn base_input(harness: &Harness, expiry_ms: i64) -> DigestInput {
    DigestInput {
        request_id: None,
        principal_id: harness.fixture.principal,
        actor_id: harness.fixture.actor,
        run_id: Some(harness.fixture.run),
        operation: "secret.use".to_owned(),
        target: TARGET.to_owned(),
        capabilities: vec![
            Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct),
            Capability::new(CapabilityFamily::Network, CapabilityAction::Connect),
        ],
        extension_digest: None,
        config_digest: None,
        expiry_ms,
        nonce: "template-nonce".to_owned(),
    }
}

fn digest_input_from_row(row: &ApprovalRequestRow) -> DigestInput {
    let capabilities = if row.capability_ids.is_empty() {
        Vec::new()
    } else {
        let text = std::str::from_utf8(&row.capability_ids).expect("capability blob is text");
        text.split('\n')
            .map(|token| Capability::from_str(token).expect("capability token"))
            .collect()
    };
    DigestInput {
        request_id: Some(row.request_id),
        principal_id: row.principal_id,
        actor_id: row.actor_id,
        run_id: row.run_id,
        operation: row.operation.clone(),
        target: row.target_resource.clone().unwrap_or_default(),
        capabilities,
        extension_digest: row.extension_bundle_digest.clone(),
        config_digest: row.config_generation_digest.clone(),
        expiry_ms: row.expires_at_ms,
        nonce: row.nonce.clone(),
    }
}

impl Harness {
    fn envelope(
        &self,
        command_type: &str,
        idempotency_key: &str,
        payload: Vec<u8>,
        marker: u8,
    ) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(idempotency_key).expect("valid key"),
            principal_id: self.fixture.principal,
            actor_id: self.fixture.actor,
            device_id: Some(self.fixture.device),
            delegation_chain_id: None,
            request_digest: request_digest(marker),
            correlation_id: Some("corr-approvals".to_owned()),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: command_type.to_owned(),
            payload,
        }
    }

    async fn try_create(
        &self,
        input: &DigestInput,
        key: &str,
        marker: u8,
    ) -> errors::Result<CommandOutcome> {
        let payload = contract::CreateApprovalRequest {
            request_id: input
                .request_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            request_digest: String::new(),
            principal_id: input.principal_id.to_string(),
            actor_id: input.actor_id.to_string(),
            run_id: input.run_id.map(|id| id.to_string()).unwrap_or_default(),
            operation: input.operation.clone(),
            target_resource: input.target.clone(),
            capability_ids: input.capabilities.iter().map(ToString::to_string).collect(),
            extension_bundle_digest: input.extension_digest.clone().unwrap_or_default(),
            config_generation_digest: input.config_digest.clone().unwrap_or_default(),
            expires_at_ms: input.expiry_ms,
            nonce: input.nonce.clone(),
        }
        .encode_to_vec();
        self.coordinator
            .execute(self.envelope(CMD_CREATE_APPROVAL_REQUEST, key, payload, marker))
            .await
    }

    async fn create(&self, input: &DigestInput, key: &str, marker: u8) -> ApprovalRequestId {
        let outcome = self
            .try_create(input, key, marker)
            .await
            .expect("create request commits");
        assert_eq!(outcome.code, OutcomeCode::Ok);
        let text = std::str::from_utf8(&outcome.payload).expect("request id is text");
        ApprovalRequestId::from_str(text).expect("outcome carries the request id")
    }

    async fn respond(
        &self,
        request_id: ApprovalRequestId,
        digest: &ApprovalDigest,
        decision: ApprovalDecision,
        responder: Responder,
        key: &str,
        marker: u8,
    ) -> errors::Result<CommandOutcome> {
        let payload = contract::RespondApproval {
            request_id: request_id.to_string(),
            request_digest: digest.to_string(),
            decision: decision.as_str().to_owned(),
            device_id: responder
                .device_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            responder_principal_id: responder.principal_id.to_string(),
        }
        .encode_to_vec();
        self.coordinator
            .execute(self.envelope(CMD_RESPOND_APPROVAL, key, payload, marker))
            .await
    }

    async fn persisted_digest(&self, request_id: ApprovalRequestId) -> ApprovalDigest {
        let row = self
            .request_row(request_id)
            .await
            .expect("request row persisted");
        persisted_digest_from(&row.request_digest)
    }

    async fn request_row(&self, request_id: ApprovalRequestId) -> Option<ApprovalRequestRow> {
        let mut txn = self.store.begin_read().await.expect("read transaction");
        txn.security()
            .get_approval_request(request_id)
            .await
            .expect("request read")
    }

    async fn responses(&self, request_id: ApprovalRequestId) -> Vec<ApprovalResponseRow> {
        let mut txn = self.store.begin_read().await.expect("read transaction");
        txn.security()
            .list_approval_responses(request_id)
            .await
            .expect("response read")
    }

    async fn staged_events(&self) -> Vec<OutboxEventRow> {
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch: self.fence_epoch,
                principal_id: self.fixture.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await
            .expect("observation transaction begins");
        let events = txn
            .streams()
            .scan_unpublished(100)
            .await
            .expect("outbox scan");
        txn.rollback().await.expect("observation rolls back");
        events
    }

    async fn outcome(
        &self,
        request_id: ApprovalRequestId,
        expected: &ApprovalDigest,
        now_ms: i64,
    ) -> ApprovalOutcome {
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch: self.fence_epoch,
                principal_id: self.fixture.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await
            .expect("resolution transaction begins");
        let outcome = is_satisfied(&mut *txn, request_id, expected, now_ms)
            .await
            .expect("resolution read succeeds");
        txn.rollback().await.expect("resolution rolls back");
        outcome
    }
}

#[test]
fn canonical_digest_matches_the_golden_framing() {
    let input = DigestInput {
        request_id: Some(
            ApprovalRequestId::from_str("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70")
                .expect("golden request id"),
        ),
        principal_id: PrincipalId::from_str("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71")
            .expect("golden principal"),
        actor_id: ActorId::from_str("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e72").expect("golden actor"),
        run_id: Some(RunId::from_str("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e73").expect("golden run")),
        operation: "secret.use".to_owned(),
        target: TARGET.to_owned(),
        capabilities: vec![
            Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct),
            Capability::new(CapabilityFamily::Network, CapabilityAction::Connect),
        ],
        extension_digest: Some("ext-digest-1".to_owned()),
        config_digest: None,
        expiry_ms: 1_700_000_900_000,
        nonce: "nonce-golden-1".to_owned(),
    };

    let digest = canonical_digest(&input);

    assert_eq!(
        digest.to_string(),
        GOLDEN_DIGEST,
        "digest framing changed; update only with a deliberate contract change"
    );
    let mut reordered = input.clone();
    reordered.capabilities.reverse();
    assert_eq!(
        canonical_digest(&reordered),
        digest,
        "capability order must be canonicalized before hashing"
    );
    assert_eq!(
        ApprovalDigest::from_str(&digest.to_string()).expect("digest round trips"),
        digest
    );

    for malformed in [
        String::new(),
        "zz".repeat(32),
        "ab".repeat(31),
        "ab".repeat(33),
        "AB".repeat(32),
    ] {
        let error = ApprovalDigest::from_str(&malformed).expect_err("malformed digest rejected");
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(
            !error.message().contains(TARGET),
            "digest errors must not echo input content: {}",
            error.message()
        );
    }
}

#[tokio::test]
async fn create_request_persists_the_request_and_stages_the_catalogued_event() {
    let harness = harness().await;
    let mut input = base_input(&harness, SEED_MS + 60_000);
    input.extension_digest = Some("ext-digest-1".to_owned());
    input.config_digest = Some("config-digest-1".to_owned());

    let request_id = harness.create(&input, "create-1", 0xa1).await;

    let row = harness
        .request_row(request_id)
        .await
        .expect("request row persisted");
    assert_eq!(row.principal_id, harness.fixture.principal);
    assert_eq!(row.actor_id, harness.fixture.actor);
    assert_eq!(row.run_id, Some(harness.fixture.run));
    assert_eq!(row.operation, "secret.use");
    assert_eq!(row.target_resource.as_deref(), Some(TARGET));
    assert_eq!(row.extension_bundle_digest.as_deref(), Some("ext-digest-1"));
    assert_eq!(
        row.config_generation_digest.as_deref(),
        Some("config-digest-1")
    );
    assert_eq!(row.expires_at_ms, SEED_MS + 60_000);
    assert_eq!(row.nonce, request_id.to_string());
    assert_eq!(row.state, domain::security::ApprovalState::Pending);
    assert_eq!(row.resolved_at_ms, None);
    assert_eq!(row.created_at_ms, SEED_MS);
    assert_eq!(
        row.request_digest,
        canonical_digest(&digest_input_from_row(&row)).to_string()
    );

    let events = harness.staged_events().await;
    assert_eq!(events.len(), 1, "exactly the catalogue event is staged");
    let event = &events[0];
    assert_eq!(event.event_type, "ApprovalRequested");
    assert_eq!(event.event_version, 1);
    assert_eq!(
        event.stream_key.as_str(),
        format!("security/principal/{}", harness.fixture.principal)
    );
    assert_eq!(event.sensitivity, SensitivityClass::Confidential);
    assert_eq!(event.retention, RetentionClass::Audit);
    let decoded = contract::ApprovalRequest::decode(event.payload.as_slice())
        .expect("ApprovalRequested payload decodes");
    assert_eq!(decoded.request_id, request_id.to_string());
    assert_eq!(decoded.request_digest, row.request_digest);
    assert_eq!(decoded.actor_id, harness.fixture.actor.to_string());
    assert_eq!(decoded.run_id, harness.fixture.run.to_string());
    assert_eq!(decoded.operation, "secret.use");
    assert_eq!(decoded.target_resource, TARGET);
    assert_eq!(decoded.expires_unix_ms, SEED_MS + 60_000);
    assert_eq!(decoded.nonce, request_id.to_string());
    assert_eq!(
        decoded.capability_ids,
        vec![
            "network.connect".to_owned(),
            "secret.sign_or_act".to_owned()
        ],
        "staged capabilities are canonicalized"
    );
}

#[tokio::test]
async fn create_binds_the_request_principal_to_the_command_context() {
    let harness = harness().await;
    let mut mismatched = base_input(&harness, SEED_MS + 60_000);
    mismatched.principal_id = harness.fixture.other_principal;

    let error = harness
        .try_create(&mismatched, "create-1", 0xa7)
        .await
        .expect_err("a payload principal outside the context is rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        harness.staged_events().await.is_empty(),
        "a rejected create stages nothing"
    );

    let matched = base_input(&harness, SEED_MS + 60_000);
    let request_id = harness.create(&matched, "create-2", 0xa8).await;
    let row = harness
        .request_row(request_id)
        .await
        .expect("request row persisted");
    assert_eq!(
        row.principal_id, harness.fixture.principal,
        "the persisted principal is the command context principal"
    );
    assert_eq!(
        row.request_digest,
        canonical_digest(&digest_input_from_row(&row)).to_string()
    );
}

#[tokio::test]
async fn response_with_a_mismatched_digest_is_rejected_and_persists_nothing() {
    let harness = harness().await;
    let input = base_input(&harness, SEED_MS + 60_000);
    let request_id = harness.create(&input, "create-1", 0xa2).await;
    let digest = harness.persisted_digest(request_id).await;

    let mut flipped = digest.to_string();
    let replacement = if flipped.starts_with('0') { "1" } else { "0" };
    flipped.replace_range(0..1, replacement);
    let wrong = persisted_digest_from(&flipped);
    assert_ne!(wrong, digest);

    let error = harness
        .respond(
            request_id,
            &wrong,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xb1,
        )
        .await
        .expect_err("digest mismatch rejected");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains(TARGET),
        "rejections must not echo target content: {}",
        error.message()
    );
    assert!(
        harness.responses(request_id).await.is_empty(),
        "a rejected response persists nothing"
    );
    assert_eq!(harness.staged_events().await.len(), 1);
}

#[tokio::test]
async fn expired_requests_are_rejected_and_persist_nothing() {
    let harness = harness().await;
    let input = base_input(&harness, SEED_MS + 1_000);
    let request_id = harness.create(&input, "create-1", 0xa3).await;
    let digest = harness.persisted_digest(request_id).await;

    harness.clock.advance(1_000);

    let error = harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xb2,
        )
        .await
        .expect_err("expired request rejected");

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains(TARGET),
        "rejections must not echo target content: {}",
        error.message()
    );
    assert!(
        harness.responses(request_id).await.is_empty(),
        "a rejected response persists nothing"
    );
}

#[tokio::test]
async fn a_second_response_is_rejected_and_leaves_the_first() {
    let harness = harness().await;
    let input = base_input(&harness, SEED_MS + 60_000);
    let request_id = harness.create(&input, "create-1", 0xa4).await;
    let digest = harness.persisted_digest(request_id).await;

    harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xb3,
        )
        .await
        .expect("first response commits");

    let error = harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Deny,
            responder(&harness),
            "respond-2",
            0xb4,
        )
        .await
        .expect_err("second response rejected");

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    let responses = harness.responses(request_id).await;
    assert_eq!(responses.len(), 1, "at most one terminal response exists");
    assert_eq!(responses[0].decision, "approve");
    assert_eq!(responses[0].request_digest, digest.to_string());
    assert_eq!(
        responses[0].responder_principal_id,
        harness.fixture.principal
    );
    assert_eq!(responses[0].device_id, harness.fixture.device);
    assert_eq!(responses[0].responded_at_ms, SEED_MS);
}

#[tokio::test]
async fn respond_stages_the_catalogued_approval_resolved_event() {
    let harness = harness().await;
    let input = base_input(&harness, SEED_MS + 60_000);
    let request_id = harness.create(&input, "create-1", 0xe3).await;
    let digest = harness.persisted_digest(request_id).await;

    harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xe4,
        )
        .await
        .expect("response commits");

    let events = harness.staged_events().await;
    assert_eq!(events.len(), 2, "requested and resolved events are staged");
    let resolved = events
        .iter()
        .find(|event| event.event_type == "ApprovalResolved")
        .expect("ApprovalResolved is staged");
    assert_eq!(resolved.event_version, 1);
    assert_eq!(
        resolved.stream_key.as_str(),
        format!("security/principal/{}", harness.fixture.principal)
    );
    assert_eq!(resolved.sensitivity, SensitivityClass::Confidential);
    assert_eq!(resolved.retention, RetentionClass::Audit);
    let decoded = ApprovalResolvedPayload::decode(resolved.payload.as_slice())
        .expect("ApprovalResolved payload decodes");
    assert_eq!(decoded.request_id, request_id.to_string());
    assert_eq!(decoded.request_digest, digest.to_string());
    assert_eq!(decoded.decision, "approve");
    assert_eq!(
        decoded.responder_principal_id,
        harness.fixture.principal.to_string()
    );
    assert_eq!(decoded.device_id, harness.fixture.device.to_string());
    assert_eq!(decoded.responded_at_ms, SEED_MS);
}

#[tokio::test]
async fn a_wrong_responder_is_rejected_and_persists_nothing() {
    let harness = harness().await;
    let input = base_input(&harness, SEED_MS + 60_000);
    let request_id = harness.create(&input, "create-1", 0xa5).await;
    let digest = harness.persisted_digest(request_id).await;

    let wrong_principal = Responder {
        principal_id: harness.fixture.other_principal,
        device_id: Some(harness.fixture.device),
    };
    let error = harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            wrong_principal,
            "respond-1",
            0xb5,
        )
        .await
        .expect_err("a payload responder outside the context is rejected");
    assert_eq!(
        error.code(),
        ErrorCode::InvalidArgument,
        "the responder principal must match the command context"
    );
    assert_eq!(error.retry_class(), RetryClass::Never);

    let missing_device = Responder {
        principal_id: harness.fixture.principal,
        device_id: None,
    };
    let error = harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            missing_device,
            "respond-2",
            0xb6,
        )
        .await
        .expect_err("missing device rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);

    assert!(
        harness.responses(request_id).await.is_empty(),
        "rejected responders persist nothing"
    );
    assert_eq!(harness.staged_events().await.len(), 1);

    harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-3",
            0xb7,
        )
        .await
        .expect("the bound responder commits");
}

#[tokio::test]
async fn changed_extension_or_config_digest_invalidates_a_prior_approval() {
    let harness = harness().await;
    let mut input = base_input(&harness, SEED_MS + 60_000);
    input.extension_digest = Some("ext-digest-1".to_owned());
    input.config_digest = Some("config-digest-1".to_owned());
    let request_id = harness.create(&input, "create-1", 0xa6).await;
    let digest = harness.persisted_digest(request_id).await;

    assert_eq!(
        harness.outcome(request_id, &digest, SEED_MS).await,
        ApprovalOutcome::Pending
    );
    harness
        .respond(
            request_id,
            &digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xb8,
        )
        .await
        .expect("approval commits");
    assert_eq!(
        harness.outcome(request_id, &digest, SEED_MS).await,
        ApprovalOutcome::Approved
    );

    let row = harness
        .request_row(request_id)
        .await
        .expect("request row persisted");
    let changed_extension = DigestInput {
        extension_digest: Some("ext-digest-2".to_owned()),
        ..digest_input_from_row(&row)
    };
    let changed_config = DigestInput {
        config_digest: Some("config-digest-2".to_owned()),
        ..digest_input_from_row(&row)
    };
    assert_ne!(canonical_digest(&changed_extension), digest);
    assert_ne!(canonical_digest(&changed_config), digest);
    assert_eq!(
        harness
            .outcome(request_id, &canonical_digest(&changed_extension), SEED_MS)
            .await,
        ApprovalOutcome::Invalidated
    );
    assert_eq!(
        harness
            .outcome(request_id, &canonical_digest(&changed_config), SEED_MS)
            .await,
        ApprovalOutcome::Invalidated
    );
}

#[tokio::test]
async fn is_satisfied_reports_each_persisted_outcome() {
    let harness = harness().await;

    let unknown = ApprovalRequestId::new(harness.ids.as_ref());
    assert_eq!(
        harness
            .outcome(
                unknown,
                &canonical_digest(&base_input(&harness, 0)),
                SEED_MS
            )
            .await,
        ApprovalOutcome::Unknown
    );

    let pending_input = base_input(&harness, SEED_MS + 1_000);
    let pending_id = harness.create(&pending_input, "create-1", 0xc1).await;
    let pending_digest = harness.persisted_digest(pending_id).await;
    assert_eq!(
        harness.outcome(pending_id, &pending_digest, SEED_MS).await,
        ApprovalOutcome::Pending
    );

    harness.clock.advance(1_000);
    assert_eq!(
        harness
            .outcome(pending_id, &pending_digest, harness.clock.now_unix_ms())
            .await,
        ApprovalOutcome::Expired
    );

    let approved_input = base_input(&harness, SEED_MS + 60_000);
    let approved_id = harness.create(&approved_input, "create-2", 0xc2).await;
    let approved_digest = harness.persisted_digest(approved_id).await;
    harness
        .respond(
            approved_id,
            &approved_digest,
            ApprovalDecision::Approve,
            responder(&harness),
            "respond-1",
            0xc3,
        )
        .await
        .expect("approval commits");
    assert_eq!(
        harness
            .outcome(
                approved_id,
                &approved_digest,
                harness.clock.now_unix_ms() + 60_000
            )
            .await,
        ApprovalOutcome::Approved,
        "a terminal response is the persisted answer"
    );

    let denied_input = base_input(&harness, SEED_MS + 120_000);
    let denied_id = harness.create(&denied_input, "create-3", 0xc4).await;
    let denied_digest = harness.persisted_digest(denied_id).await;
    harness
        .respond(
            denied_id,
            &denied_digest,
            ApprovalDecision::Deny,
            responder(&harness),
            "respond-2",
            0xc5,
        )
        .await
        .expect("denial commits");
    assert_eq!(
        harness
            .outcome(denied_id, &denied_digest, harness.clock.now_unix_ms())
            .await,
        ApprovalOutcome::Denied
    );
}

#[tokio::test]
async fn malformed_respond_payloads_are_rejected_without_rows() {
    let harness = harness().await;
    let payload = b"not-a-prost-payload".to_vec();

    let error = harness
        .coordinator
        .execute(harness.envelope(CMD_RESPOND_APPROVAL, "respond-1", payload, 0xd1))
        .await
        .expect_err("malformed payload rejected");

    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains("not-a-prost"),
        "rejections must not echo payload bytes: {}",
        error.message()
    );
    assert_eq!(harness.staged_events().await.len(), 0);
}
