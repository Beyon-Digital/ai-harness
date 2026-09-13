//! Command coordinator acceptance tests over a real SQLite kernel store:
//! fresh commits, idempotent replay, digest conflicts, both commit fault
//! points, fence staleness, registry rejection, and envelope validation
//! (R1-R4, N1, P1).

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{
    CommandContext, CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode,
};
use command_coordinator::{AFTER_COMMIT, BEFORE_COMMIT, CommandCoordinator, FixedFence};
use domain::ids::{
    ActorId, CommandId, DaemonInstanceId, EventId, EventStreamKey, IdempotencyKey, PrincipalId,
    SessionId,
};
use domain::security::{RetentionClass, SensitivityClass};
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::outbox::{DraftEvent, stage};
use kernel_store::models::{IdempotencyRecordRow, NewSession, OutboxEventRow, SessionRow};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";
const TEST_COMMAND: &str = "session.create";
const FAILING_COMMAND: &str = "session.fail";
const PAYLOAD_MARKER: &str = "do-not-echo-payload-marker-7c1f";

fn session_id_of(command_id: CommandId) -> SessionId {
    SessionId::from_uuid_v7(*command_id.as_uuid_v7())
}

/// Representative handler: one session row, one staged outbox event, and an
/// `ok` outcome payload.
struct TestHandler {
    clock: Arc<TestClock>,
    calls: AtomicUsize,
}

impl TestHandler {
    fn new(clock: Arc<TestClock>) -> Self {
        Self {
            clock,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CommandHandler for TestHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let session_id = session_id_of(ctx.command_id);
        txn.sessions()
            .insert(NewSession {
                session_id,
                principal_id: ctx.principal_id,
                created_at_ms: self.clock.now_unix_ms(),
                metadata: None,
            })
            .await?;
        let event_id = EventId::from_uuid_v7(*ctx.command_id.as_uuid_v7());
        let stream_key = EventStreamKey::new(format!("session/{session_id}")).map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "representative handler built an invalid stream key",
            )
        })?;
        stage(
            txn,
            DraftEvent {
                event_id,
                stream_key,
                event_type: "session.created".to_owned(),
                payload,
                sensitivity: SensitivityClass::Private,
                retention: RetentionClass::Durable,
                correlation_id: ctx.correlation_id.clone(),
                causation_id: None,
            },
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: b"created".to_vec(),
        })
    }
}

/// Failing handler: stages a session and one outbox event, then returns
/// `Internal` so the coordinator's early `?` path must roll everything back.
struct FailingHandler;

#[async_trait]
impl CommandHandler for FailingHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let session_id = session_id_of(ctx.command_id);
        txn.sessions()
            .insert(NewSession {
                session_id,
                principal_id: ctx.principal_id,
                created_at_ms: 0,
                metadata: None,
            })
            .await?;
        let event_id = EventId::from_uuid_v7(*ctx.command_id.as_uuid_v7());
        let stream_key = EventStreamKey::new(format!("session/{session_id}")).map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "failing handler built an invalid stream key",
            )
        })?;
        stage(
            txn,
            DraftEvent {
                event_id,
                stream_key,
                event_type: "session.failed".to_owned(),
                payload,
                sensitivity: SensitivityClass::Private,
                retention: RetentionClass::Durable,
                correlation_id: ctx.correlation_id.clone(),
                causation_id: None,
            },
        )
        .await?;
        Err(KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "failing handler aborted after staging work",
        ))
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    coordinator: CommandCoordinator,
    clock: Arc<TestClock>,
    faults: Arc<ArmedFaults>,
    handler: Arc<TestHandler>,
    fence_epoch: u64,
}

#[derive(Debug, PartialEq)]
struct Observation {
    session: Option<SessionRow>,
    idempotency: Option<IdempotencyRecordRow>,
    unpublished: Vec<OutboxEventRow>,
}

impl Harness {
    async fn observe(&self, envelope: &CommandEnvelope) -> Observation {
        self.observe_at(self.fence_epoch, envelope).await
    }

    async fn observe_at(&self, daemon_epoch: u64, envelope: &CommandEnvelope) -> Observation {
        let mut txn = self
            .store
            .begin_write(TxContext {
                daemon_epoch,
                principal_id: envelope.principal_id,
                command_id: envelope.command_id,
                correlation_id: None,
            })
            .await
            .expect("observation transaction begins under a live fence");
        let session = txn
            .sessions()
            .get(session_id_of(envelope.command_id))
            .await
            .unwrap();
        let idempotency = txn
            .idempotency()
            .lookup(envelope.principal_id, &envelope.idempotency_key)
            .await
            .unwrap();
        let unpublished = txn.streams().scan_unpublished(100).await.unwrap();
        txn.rollback().await.unwrap();
        Observation {
            session,
            idempotency,
            unpublished,
        }
    }
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE);
    let store = Arc::new(
        SqliteKernelStore::open(StoreConfig {
            path,
            pool_max_connections: 5,
            busy_timeout_ms: 5_000,
        })
        .await
        .unwrap(),
    );
    let provider = DeterministicIds::new(SEED_MS);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    let clock = Arc::new(TestClock::new(SEED_MS));
    let faults = Arc::new(ArmedFaults::new());
    let handler = Arc::new(TestHandler::new(clock.clone()));
    let mut registry = CommandRegistry::new();
    registry.register(TEST_COMMAND, handler.clone()).unwrap();
    registry
        .register(FAILING_COMMAND, Arc::new(FailingHandler))
        .unwrap();
    let coordinator = CommandCoordinator::new(
        store.clone(),
        Arc::new(registry),
        Arc::new(FixedFence(fence.epoch.0)),
        clock.clone(),
        faults.clone(),
    );
    Harness {
        _dir: dir,
        store,
        coordinator,
        clock,
        faults,
        handler,
        fence_epoch: fence.epoch.0,
    }
}

fn envelope(command_type: &str, request_digest: RequestDigest) -> CommandEnvelope {
    let provider = DeterministicIds::new(SEED_MS);
    CommandEnvelope {
        command_id: CommandId::new(&provider),
        idempotency_key: IdempotencyKey::new("command-1").unwrap(),
        principal_id: PrincipalId::new(&provider),
        actor_id: ActorId::new(&provider),
        device_id: None,
        delegation_chain_id: None,
        request_digest,
        correlation_id: Some("corr-1".to_owned()),
        causation_id: None,
        deadline_unix_ms: None,
        command_type: command_type.to_owned(),
        payload: PAYLOAD_MARKER.as_bytes().to_vec(),
    }
}

fn digest(byte: u8) -> RequestDigest {
    RequestDigest::from_str(&format!("{byte:02x}").repeat(32)).expect("valid digest")
}

#[tokio::test]
async fn fresh_command_commits_rows_outbox_and_idempotency() {
    let harness = harness().await;
    let envelope = envelope(TEST_COMMAND, digest(0xa1));

    let outcome = harness.coordinator.execute(envelope.clone()).await.unwrap();

    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(outcome.payload, b"created");

    let observed = harness.observe(&envelope).await;
    let session = observed.session.expect("session row committed");
    assert_eq!(session.principal_id, envelope.principal_id);
    assert_eq!(session.created_at_ms, harness.clock.now_unix_ms());
    let record = observed.idempotency.expect("idempotency record committed");
    assert_eq!(record.principal_id, envelope.principal_id);
    assert_eq!(record.idempotency_key, envelope.idempotency_key);
    assert_eq!(record.command_id, envelope.command_id);
    assert_eq!(record.request_digest, envelope.request_digest.to_string());
    assert_eq!(record.outcome_code, "ok");
    assert_eq!(record.outcome_payload, b"created");
    assert_eq!(record.created_at_ms, harness.clock.now_unix_ms());
    assert_eq!(observed.unpublished.len(), 1);
    assert_eq!(observed.unpublished[0].event_type, "session.created");
    assert_eq!(
        observed.unpublished[0].payload,
        PAYLOAD_MARKER.as_bytes(),
        "staged event must carry the envelope payload"
    );
    assert_eq!(harness.handler.calls(), 1);
}

#[tokio::test]
async fn identical_replay_returns_the_stored_outcome_without_changes() {
    let harness = harness().await;
    let envelope = envelope(TEST_COMMAND, digest(0xa2));

    let first = harness.coordinator.execute(envelope.clone()).await.unwrap();
    let mut observation = harness.observe(&envelope).await;

    for replay_index in 0..3 {
        let replayed = harness.coordinator.execute(envelope.clone()).await.unwrap();
        assert_eq!(
            replayed, first,
            "replay {replay_index} returned a different outcome"
        );
        let next = harness.observe(&envelope).await;
        assert_eq!(
            next, observation,
            "replay {replay_index} changed persisted state"
        );
        observation = next;
    }
    assert_eq!(
        harness.handler.calls(),
        1,
        "replay must not run the handler"
    );
}

#[tokio::test]
async fn different_digest_for_the_same_key_is_a_conflict() {
    let harness = harness().await;
    let envelope = envelope(TEST_COMMAND, digest(0xb1));
    harness.coordinator.execute(envelope.clone()).await.unwrap();
    let before = harness.observe(&envelope).await;

    let mut conflicting = envelope.clone();
    conflicting.request_digest = digest(0xb2);
    let error = harness.coordinator.execute(conflicting).await.unwrap_err();

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert_eq!(
        harness.handler.calls(),
        1,
        "conflict must not run the handler"
    );
    assert_eq!(harness.observe(&envelope).await, before);
}

#[tokio::test]
async fn pre_commit_fault_rolls_back_and_resubmission_succeeds() {
    let harness = harness().await;
    let envelope = envelope(TEST_COMMAND, digest(0xc1));
    harness.faults.arm(BEFORE_COMMIT);

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Unavailable);
    assert_eq!(error.retry_class(), RetryClass::Safe);
    harness.faults.assert_triggered(BEFORE_COMMIT);
    let rolled_back = harness.observe(&envelope).await;
    assert!(rolled_back.session.is_none());
    assert!(rolled_back.idempotency.is_none());
    assert!(rolled_back.unpublished.is_empty());

    let outcome = harness.coordinator.execute(envelope.clone()).await.unwrap();

    assert_eq!(outcome.payload, b"created");
    let committed = harness.observe(&envelope).await;
    assert!(committed.session.is_some());
    assert!(committed.idempotency.is_some());
    assert_eq!(committed.unpublished.len(), 1);
    assert_eq!(harness.handler.calls(), 2);
}

#[tokio::test]
async fn post_commit_fault_reports_unavailable_then_replay_returns_the_outcome() {
    let harness = harness().await;
    let envelope = envelope(TEST_COMMAND, digest(0xd1));
    harness.faults.arm(AFTER_COMMIT);

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Unavailable);
    assert_eq!(error.retry_class(), RetryClass::Safe);
    harness.faults.assert_triggered(AFTER_COMMIT);
    let committed = harness.observe(&envelope).await;
    assert!(
        committed.session.is_some(),
        "work committed before the fault"
    );
    assert!(committed.idempotency.is_some());
    assert_eq!(committed.unpublished.len(), 1);

    let replayed = harness.coordinator.execute(envelope.clone()).await.unwrap();

    assert_eq!(replayed.code, OutcomeCode::Ok);
    assert_eq!(replayed.payload, b"created");
    assert_eq!(
        harness.handler.calls(),
        1,
        "replay must not rerun the handler"
    );
    assert_eq!(harness.observe(&envelope).await, committed);
}

#[tokio::test]
async fn handler_error_rolls_back_every_staged_write() {
    let harness = harness().await;
    let envelope = envelope(FAILING_COMMAND, digest(0x9a));

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains(PAYLOAD_MARKER),
        "handler error must not echo the payload: {}",
        error.message()
    );
    assert_eq!(
        harness.observe(&envelope).await,
        Observation {
            session: None,
            idempotency: None,
            unpublished: Vec::new(),
        },
        "handler error must roll back sessions, outbox events, and idempotency"
    );
}

#[tokio::test]
async fn stale_fence_rejects_without_rows() {
    let harness = harness().await;
    let provider = DeterministicIds::new(SEED_MS + 1);
    let advanced = harness
        .store
        .acquire_daemon_fence(DaemonInstanceId::new(&provider))
        .await
        .unwrap();
    assert_eq!(advanced.epoch.0, harness.fence_epoch + 1);

    let envelope = envelope(TEST_COMMAND, digest(0xe1));
    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert_eq!(harness.handler.calls(), 0);
    let observed = harness.observe_at(advanced.epoch.0, &envelope).await;
    assert!(observed.session.is_none());
    assert!(observed.idempotency.is_none());
    assert!(observed.unpublished.is_empty());
}

#[tokio::test]
async fn unknown_command_type_rejects_without_rows() {
    let harness = harness().await;
    let envelope = envelope("session.unknown", digest(0xf1));

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        error.message().contains("session.unknown"),
        "message must name the unregistered command_type: {}",
        error.message()
    );
    assert!(
        !error.message().contains(PAYLOAD_MARKER),
        "rejection must not echo the payload: {}",
        error.message()
    );
    assert_eq!(harness.handler.calls(), 0);
    let observed = harness.observe(&envelope).await;
    assert!(observed.session.is_none());
    assert!(observed.idempotency.is_none());
    assert!(observed.unpublished.is_empty());
}

#[tokio::test]
async fn duplicate_registration_is_a_conflict() {
    let clock = Arc::new(TestClock::new(SEED_MS));
    let handler = Arc::new(TestHandler::new(clock));
    let mut registry = CommandRegistry::new();
    registry.register(TEST_COMMAND, handler.clone()).unwrap();

    let error = registry
        .register(TEST_COMMAND, handler.clone())
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains(PAYLOAD_MARKER),
        "rejection must not echo the payload: {}",
        error.message()
    );
    assert_eq!(registry.len(), 1);
    assert!(registry.get(TEST_COMMAND).is_some());
}

#[tokio::test]
async fn malformed_digest_is_rejected() {
    let cases = [
        String::new(),
        "zz".repeat(32),
        "ab".repeat(31),
        "ab".repeat(33),
        "AB".repeat(32),
    ];
    for case in cases {
        let error = RequestDigest::from_str(&case).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArgument);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(
            !error.message().contains(PAYLOAD_MARKER),
            "rejection must not echo the payload: {}",
            error.message()
        );
    }
}

#[tokio::test]
async fn past_deadline_is_rejected_without_rows() {
    let harness = harness().await;
    let mut envelope = envelope(TEST_COMMAND, digest(0x42));
    envelope.deadline_unix_ms = Some(harness.clock.now_unix_ms() - 1);

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains(PAYLOAD_MARKER),
        "rejection must not echo the payload: {}",
        error.message()
    );
    assert_eq!(harness.handler.calls(), 0);
    let observed = harness.observe(&envelope).await;
    assert!(observed.session.is_none());
    assert!(observed.idempotency.is_none());
    assert!(observed.unpublished.is_empty());

    envelope.deadline_unix_ms = Some(harness.clock.now_unix_ms());
    let outcome = harness.coordinator.execute(envelope.clone()).await.unwrap();

    assert_eq!(outcome.payload, b"created");
}
