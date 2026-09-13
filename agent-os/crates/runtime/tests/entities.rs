//! Entity creation acceptance tests over the real coordinator and the real
//! SQLite kernel store: immutable agent spec revisions, idempotent session and
//! task creation, child parent linkage, cancellation-epoch fencing, and
//! `Created` runs with outbox and idempotency rows (R1, N1).

use std::str::FromStr;
use std::sync::Arc;

use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use domain::generated::contract;
use domain::ids::{
    ActorId, AgentSpecId, CommandId, DaemonInstanceId, EventId, IdempotencyKey, PrincipalId, RunId,
    SessionId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::{RetentionClass, SensitivityClass};
use domain::time::Clock;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{
    AgentSpecRow, NewSession, OutboxEventRow, RunCas, RunGraphHeadRow, RunPatch, RunRow,
    SessionRow, TaskRow,
};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use runtime::{CMD_CREATE_SESSION, CMD_CREATE_TASK_RUN, CMD_PUT_AGENT_SPEC_REVISION, RuntimeDeps};
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const DB_FILE: &str = "kernel.db";

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    coordinator: CommandCoordinator,
    clock: Arc<TestClock>,
    ids: Arc<DeterministicIds>,
    principal: PrincipalId,
    actor: ActorId,
    fence_epoch: u64,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp root");
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
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(ids.as_ref()))
            .await
            .expect("daemon fence acquired");
        let clock = Arc::new(TestClock::new(SEED_MS));
        let mut registry = CommandRegistry::new();
        runtime::register_handlers(
            &mut registry,
            RuntimeDeps {
                clock: clock.clone(),
                ids: ids.clone(),
            },
        )
        .expect("runtime handlers register");
        let coordinator = CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            clock.clone(),
            Arc::new(ArmedFaults::new()),
        );
        Self {
            _dir: dir,
            store,
            coordinator,
            clock,
            principal: PrincipalId::new(ids.as_ref()),
            actor: ActorId::new(ids.as_ref()),
            ids,
            fence_epoch: fence.epoch.0,
        }
    }

    fn envelope(&self, command_type: &str, key: &str, payload: Vec<u8>) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(key).expect("valid idempotency key"),
            principal_id: self.principal,
            actor_id: self.actor,
            device_id: None,
            delegation_chain_id: None,
            request_digest: digest(),
            correlation_id: Some("corr-1".to_owned()),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: command_type.to_owned(),
            payload,
        }
    }

    async fn write_txn(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(TxContext {
                daemon_epoch: self.fence_epoch,
                principal_id: self.principal,
                command_id: CommandId::new(self.ids.as_ref()),
                correlation_id: None,
            })
            .await
            .expect("write transaction opens")
    }

    async fn session(&self, id: SessionId) -> Option<SessionRow> {
        let mut txn = self.write_txn().await;
        let row = txn.sessions().get(id).await.expect("session read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn task(&self, id: TaskId) -> Option<TaskRow> {
        let mut txn = self.write_txn().await;
        let row = txn.tasks().get(id).await.expect("task read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn run(&self, id: RunId) -> Option<RunRow> {
        let mut txn = self.write_txn().await;
        let row = txn.runs().get(id).await.expect("run read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn head(&self, id: TaskId) -> Option<RunGraphHeadRow> {
        let mut txn = self.write_txn().await;
        let row = txn.graph().get_head(id).await.expect("head read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn spec(&self, id: AgentSpecId, version: &str) -> Option<AgentSpecRow> {
        let mut txn = self.write_txn().await;
        let row = txn.agent_specs().get(id, version).await.expect("spec read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn record(
        &self,
        envelope: &CommandEnvelope,
    ) -> Option<kernel_store::models::IdempotencyRecordRow> {
        let mut txn = self.write_txn().await;
        let row = txn
            .idempotency()
            .lookup(envelope.principal_id, &envelope.idempotency_key)
            .await
            .expect("idempotency read");
        txn.rollback().await.expect("rollback");
        row
    }

    async fn events(&self) -> Vec<OutboxEventRow> {
        let mut txn = self.write_txn().await;
        let rows = txn
            .streams()
            .scan_unpublished(1_000)
            .await
            .expect("outbox read");
        txn.rollback().await.expect("rollback");
        rows
    }

    async fn seed_session(&self, session_id: SessionId) {
        let mut txn = self.write_txn().await;
        txn.sessions()
            .insert(NewSession {
                session_id,
                principal_id: self.principal,
                created_at_ms: self.clock.now_unix_ms(),
                metadata: None,
            })
            .await
            .expect("seed session");
        txn.commit().await.expect("seed session commits");
    }

    async fn seed_spec(
        &self,
        agent_spec_id: AgentSpecId,
        version: &str,
        body: &[u8],
        body_digest: &str,
    ) {
        let mut txn = self.write_txn().await;
        txn.agent_specs()
            .insert(kernel_store::models::NewAgentSpec {
                agent_spec_id,
                version: version.to_owned(),
                digest: body_digest.to_owned(),
                body: body.to_vec(),
                created_at_ms: self.clock.now_unix_ms(),
            })
            .await
            .expect("seed spec");
        txn.commit().await.expect("seed spec commits");
    }

    async fn advance_parent_cancellation_epoch(&self, parent: RunId) {
        let mut txn = self.write_txn().await;
        let updated = txn
            .runs()
            .cas_update(
                parent,
                RunCas {
                    run_revision: 0,
                    state: None,
                    cancellation_epoch: None,
                },
                RunPatch {
                    cancellation_epoch: Some(1),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await
            .expect("parent epoch advance");
        assert!(updated, "parent cancellation epoch advances");
        txn.commit().await.expect("epoch advance commits");
    }
}

fn digest() -> RequestDigest {
    RequestDigest::from_str(&"ab".repeat(32)).expect("valid digest")
}

fn create_session_payload(session_id: SessionId, metadata: Vec<u8>) -> Vec<u8> {
    contract::CreateSession {
        session_id: session_id.to_string(),
        metadata,
    }
    .encode_to_vec()
}

fn put_spec_payload(
    agent_spec_id: AgentSpecId,
    version: &str,
    body: &[u8],
    body_digest: &str,
) -> Vec<u8> {
    contract::PutAgentSpecRevision {
        agent_spec_id: agent_spec_id.to_string(),
        version: version.to_owned(),
        body_bytes: body.to_vec(),
        body_digest: body_digest.to_owned(),
    }
    .encode_to_vec()
}

#[allow(clippy::too_many_arguments)]
fn create_task_run_payload(
    task_id: TaskId,
    run_id: RunId,
    session_id: Option<SessionId>,
    task_kind: &str,
    task_payload: &[u8],
    agent_spec_ref: Option<&AgentSpecRow>,
    parent_run_id: Option<RunId>,
    observed_parent_cancellation_epoch: u64,
) -> Vec<u8> {
    contract::CreateTaskRun {
        task_id: task_id.to_string(),
        run_id: run_id.to_string(),
        session_id: session_id.map(|id| id.to_string()).unwrap_or_default(),
        task_kind: task_kind.to_owned(),
        task_payload: task_payload.to_vec(),
        agent_spec_ref: agent_spec_ref.map(|spec| contract::VersionedRef {
            id: spec.agent_spec_id.to_string(),
            version: spec.version.clone(),
            digest: spec.digest.clone(),
        }),
        parent_run_id: parent_run_id.map(|id| id.to_string()).unwrap_or_default(),
        observed_parent_cancellation_epoch,
        requested_profile: "default".to_owned(),
        workspace_uri: "file:///workspace".to_owned(),
        requested_capabilities: vec!["model".to_owned()],
        requested_budget: vec![1, 2, 3],
    }
    .encode_to_vec()
}

fn events_of_type<'a>(events: &'a [OutboxEventRow], event_type: &str) -> Vec<&'a OutboxEventRow> {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .collect()
}

#[tokio::test]
async fn create_session_binds_principal_and_stages_event() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    let envelope = harness.envelope(
        CMD_CREATE_SESSION,
        "session-1",
        create_session_payload(session_id, b"meta".to_vec()),
    );

    let outcome = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect("session creation succeeds");

    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(outcome.payload, session_id.to_string().into_bytes());
    let session = harness
        .session(session_id)
        .await
        .expect("session persisted");
    assert_eq!(session.principal_id, harness.principal);
    assert_eq!(session.created_at_ms, harness.clock.now_unix_ms());
    assert_eq!(session.metadata.as_deref(), None);

    let record = harness
        .record(&envelope)
        .await
        .expect("idempotency record persisted");
    assert_eq!(record.outcome_code, "ok");
    assert_eq!(record.outcome_payload, outcome.payload);

    let events = harness.events().await;
    let created = events_of_type(&events, "SessionCreated");
    assert_eq!(created.len(), 1);
    assert_eq!(
        created[0].stream_key.as_str(),
        format!("session/{session_id}")
    );
    assert_eq!(created[0].sequence, 1);
    assert_eq!(created[0].sensitivity, SensitivityClass::Internal);
    assert_eq!(created[0].retention, RetentionClass::Standard);
    let payload = contract::Session::decode(created[0].payload.as_slice())
        .expect("SessionCreated payload decodes");
    assert_eq!(payload.session_id, session_id.to_string());
    assert_eq!(payload.principal_id, harness.principal.to_string());
}

#[tokio::test]
async fn create_session_without_id_derives_it_from_the_command() {
    let harness = Harness::new().await;
    let mut envelope = harness.envelope(CMD_CREATE_SESSION, "session-derived", Vec::new());
    envelope.payload = contract::CreateSession {
        session_id: String::new(),
        metadata: Vec::new(),
    }
    .encode_to_vec();

    let outcome = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect("session creation succeeds");

    assert_eq!(outcome.code, OutcomeCode::Ok);
    let derived = SessionId::from_str(
        std::str::from_utf8(&outcome.payload).expect("outcome payload is the session id"),
    )
    .expect("derived session id parses");
    let session = harness.session(derived).await.expect("session persisted");
    assert_eq!(session.principal_id, harness.principal);
    assert!(harness.record(&envelope).await.is_some());
}

#[tokio::test]
async fn replayed_create_session_returns_the_stored_outcome_without_new_events() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    let envelope = harness.envelope(
        CMD_CREATE_SESSION,
        "session-replay",
        create_session_payload(session_id, Vec::new()),
    );

    let first = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect("first execution");
    for _ in 0..3 {
        let replay = harness
            .coordinator
            .execute(envelope.clone())
            .await
            .expect("replay succeeds");
        assert_eq!(replay, first);
    }

    let events = harness.events().await;
    assert_eq!(events_of_type(&events, "SessionCreated").len(), 1);
    assert_eq!(
        harness
            .session(session_id)
            .await
            .expect("row")
            .created_at_ms,
        SEED_MS
    );
}

#[tokio::test]
async fn agent_spec_revisions_are_idempotent_and_immutable() {
    let harness = Harness::new().await;
    let spec_id = AgentSpecId::new(harness.ids.as_ref());
    let body = br#"{"model":"alpha"}"#.to_vec();
    let body_digest = "a1".repeat(32);
    let original = harness.envelope(
        CMD_PUT_AGENT_SPEC_REVISION,
        "spec-1",
        put_spec_payload(spec_id, "1", &body, &body_digest),
    );

    harness
        .coordinator
        .execute(original.clone())
        .await
        .expect("first revision stores");

    let stored = harness.spec(spec_id, "1").await.expect("revision stored");
    assert_eq!(stored.body, body);
    assert_eq!(stored.digest, body_digest);
    assert_eq!(stored.created_at_ms, SEED_MS);
    let events = harness.events().await;
    let stored_events = events_of_type(&events, "AgentSpecRevisionStored");
    assert_eq!(stored_events.len(), 1);
    assert_eq!(stored_events[0].stream_key.as_str(), "config/global");
    assert_eq!(stored_events[0].sequence, 1);
    assert_eq!(stored_events[0].sensitivity, SensitivityClass::Internal);
    assert_eq!(stored_events[0].retention, RetentionClass::Audit);
    let payload = contract::AgentSpecRevision::decode(stored_events[0].payload.as_slice())
        .expect("AgentSpecRevisionStored payload decodes");
    assert_eq!(payload.agent_spec_id, spec_id.to_string());
    assert_eq!(payload.digest, body_digest);

    // Identical restatement under a fresh command is idempotent and stages
    // nothing new.
    let restated = harness.envelope(
        CMD_PUT_AGENT_SPEC_REVISION,
        "spec-1-again",
        put_spec_payload(spec_id, "1", &body, &body_digest),
    );
    harness
        .coordinator
        .execute(restated.clone())
        .await
        .expect("identical restatement succeeds");
    let after = harness.spec(spec_id, "1").await.expect("revision stored");
    assert_eq!(after, stored);
    let events = harness.events().await;
    assert_eq!(events_of_type(&events, "AgentSpecRevisionStored").len(), 1);

    // Different bytes for the same version conflict and leave the stored
    // revision untouched.
    let conflicting = harness.envelope(
        CMD_PUT_AGENT_SPEC_REVISION,
        "spec-1-conflict",
        put_spec_payload(spec_id, "1", b"different", &"b2".repeat(32)),
    );
    let error = harness
        .coordinator
        .execute(conflicting.clone())
        .await
        .expect_err("divergent revision conflicts");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(!error.message().contains("different"));
    assert_eq!(harness.spec(spec_id, "1").await.expect("row"), stored);
    let events = harness.events().await;
    assert_eq!(events_of_type(&events, "AgentSpecRevisionStored").len(), 1);
    assert!(harness.record(&conflicting).await.is_none());
}

#[tokio::test]
async fn create_task_run_persists_task_head_and_created_run() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let spec_id = AgentSpecId::new(harness.ids.as_ref());
    let spec_body = b"spec-body";
    harness
        .seed_spec(spec_id, "1", spec_body, &"c3".repeat(32))
        .await;
    let spec = harness.spec(spec_id, "1").await.expect("seeded spec");
    let task_id = TaskId::new(harness.ids.as_ref());
    let run_id = RunId::new(harness.ids.as_ref());
    let envelope = harness.envelope(
        CMD_CREATE_TASK_RUN,
        "run-1",
        create_task_run_payload(
            task_id,
            run_id,
            Some(session_id),
            "chat",
            b"task-payload",
            Some(&spec),
            None,
            0,
        ),
    );

    let outcome = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect("run creation succeeds");

    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(outcome.payload, run_id.to_string().into_bytes());
    let run = harness.run(run_id).await.expect("run persisted");
    assert_eq!(run.state, RunState::Created);
    assert_ne!(run.state, RunState::Ready);
    assert_eq!(run.recovery, RecoveryDisposition::Normal);
    assert_eq!(run.task_id, task_id);
    assert_eq!(run.session_id, Some(session_id));
    assert_eq!(run.parent_run_id, None);
    assert_eq!(run.run_revision, 0);
    assert_eq!(run.cancellation_epoch, 0);
    assert_eq!(run.resolved_environment_id, None);
    assert_eq!(run.created_at_ms, SEED_MS);
    assert_eq!(run.updated_at_ms, SEED_MS);
    let task = harness.task(task_id).await.expect("task persisted");
    assert_eq!(task.task_kind, "chat");
    assert_eq!(task.payload, b"task-payload");
    assert_eq!(task.session_id, Some(session_id));
    assert_eq!(task.created_by_actor_id, harness.actor);
    let head = harness.head(task_id).await.expect("graph head persisted");
    assert_eq!(head.graph_revision, 0);

    let record = harness
        .record(&envelope)
        .await
        .expect("idempotency record persisted");
    assert_eq!(record.outcome_payload, outcome.payload);
    let events = harness.events().await;
    let created_tasks = events_of_type(&events, "TaskCreated");
    assert_eq!(created_tasks.len(), 1);
    assert_eq!(
        created_tasks[0].stream_key.as_str(),
        format!("task/{task_id}")
    );
    assert_eq!(created_tasks[0].sequence, 1);
    assert_eq!(created_tasks[0].sensitivity, SensitivityClass::Internal);
    assert_eq!(created_tasks[0].retention, RetentionClass::Standard);
    let task_payload = contract::Task::decode(created_tasks[0].payload.as_slice())
        .expect("TaskCreated payload decodes");
    assert_eq!(task_payload.task_id, task_id.to_string());
    assert_eq!(task_payload.task_kind, "chat");
    let created_runs = events_of_type(&events, "RunCreated");
    assert_eq!(created_runs.len(), 1);
    assert_eq!(created_runs[0].stream_key.as_str(), format!("run/{run_id}"));
    assert_eq!(created_runs[0].sequence, 1);
    let run_payload = contract::AgentRun::decode(created_runs[0].payload.as_slice())
        .expect("RunCreated payload decodes");
    assert_eq!(run_payload.run_id, run_id.to_string());
    assert_eq!(run_payload.state, RunState::Created.to_wire());
    assert_eq!(run_payload.recovery, RecoveryDisposition::Normal.to_wire());
}

#[tokio::test]
async fn second_run_on_the_same_task_does_not_recreate_it() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let task_id = TaskId::new(harness.ids.as_ref());
    let first_run = RunId::new(harness.ids.as_ref());
    let second_run = RunId::new(harness.ids.as_ref());

    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "run-1",
            create_task_run_payload(
                task_id,
                first_run,
                Some(session_id),
                "chat",
                b"p",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("first run succeeds");
    let task_before = harness.task(task_id).await.expect("task persisted");
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "run-2",
            create_task_run_payload(
                task_id,
                second_run,
                Some(session_id),
                "chat",
                b"p2",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("second run succeeds");

    assert_eq!(harness.task(task_id).await.expect("task"), task_before);
    assert_eq!(harness.head(task_id).await.expect("head").graph_revision, 0);
    assert_eq!(
        harness.run(first_run).await.expect("run").state,
        RunState::Created
    );
    assert_eq!(
        harness.run(second_run).await.expect("run").state,
        RunState::Created
    );
    let events = harness.events().await;
    assert_eq!(
        events_of_type(&events, "TaskCreated").len(),
        1,
        "the existing task is not re-announced"
    );
    assert_eq!(events_of_type(&events, "RunCreated").len(), 2);
}

#[tokio::test]
async fn child_run_records_parent_linkage_and_stages_child_created() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let task_id = TaskId::new(harness.ids.as_ref());
    let parent = RunId::new(harness.ids.as_ref());
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "parent",
            create_task_run_payload(
                task_id,
                parent,
                Some(session_id),
                "chat",
                b"p",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("parent creation succeeds");

    let child = RunId::new(harness.ids.as_ref());
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "child",
            create_task_run_payload(
                task_id,
                child,
                Some(session_id),
                "chat",
                b"c",
                None,
                Some(parent),
                0,
            ),
        ))
        .await
        .expect("child creation succeeds");

    let child_row = harness.run(child).await.expect("child persisted");
    assert_eq!(child_row.parent_run_id, Some(parent));
    assert_eq!(child_row.state, RunState::Created);
    let parent_row = harness.run(parent).await.expect("parent persisted");
    assert_eq!(parent_row.cancellation_epoch, 0);
    let events = harness.events().await;
    let child_events = events_of_type(&events, "ChildRunCreated");
    assert_eq!(child_events.len(), 1);
    assert_eq!(child_events[0].stream_key.as_str(), format!("run/{parent}"));
    assert_eq!(child_events[0].sequence, 2);
    let payload = contract::AgentRun::decode(child_events[0].payload.as_slice())
        .expect("ChildRunCreated payload decodes");
    assert_eq!(payload.run_id, child.to_string());
    assert_eq!(payload.parent_run_id, parent.to_string());
}

#[tokio::test]
async fn stale_or_absent_parent_epoch_is_rejected_without_rows() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let task_id = TaskId::new(harness.ids.as_ref());
    let parent = RunId::new(harness.ids.as_ref());
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "parent",
            create_task_run_payload(
                task_id,
                parent,
                Some(session_id),
                "chat",
                b"p",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("parent creation succeeds");
    harness.advance_parent_cancellation_epoch(parent).await;
    let events_before = harness.events().await;

    let stale_child = RunId::new(harness.ids.as_ref());
    let stale_task = TaskId::new(harness.ids.as_ref());
    let stale_envelope = harness.envelope(
        CMD_CREATE_TASK_RUN,
        "stale-child",
        create_task_run_payload(
            stale_task,
            stale_child,
            Some(session_id),
            "chat",
            b"c",
            None,
            Some(parent),
            0,
        ),
    );
    let error = harness
        .coordinator
        .execute(stale_envelope.clone())
        .await
        .expect_err("stale observed epoch is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.run(stale_child).await.is_none());
    assert!(harness.task(stale_task).await.is_none());
    assert_eq!(harness.events().await.len(), events_before.len());
    assert!(harness.record(&stale_envelope).await.is_none());

    let absent_parent = RunId::new(harness.ids.as_ref());
    let absent_envelope = harness.envelope(
        CMD_CREATE_TASK_RUN,
        "absent-parent",
        create_task_run_payload(
            TaskId::new(harness.ids.as_ref()),
            RunId::new(harness.ids.as_ref()),
            Some(session_id),
            "chat",
            b"c",
            None,
            Some(absent_parent),
            0,
        ),
    );
    let error = harness
        .coordinator
        .execute(absent_envelope.clone())
        .await
        .expect_err("absent parent is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.record(&absent_envelope).await.is_none());
    assert_eq!(harness.events().await.len(), events_before.len());
}

#[tokio::test]
async fn unknown_or_mismatched_agent_spec_reference_is_rejected() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let spec_id = AgentSpecId::new(harness.ids.as_ref());
    harness
        .seed_spec(spec_id, "1", b"body", &"d4".repeat(32))
        .await;
    let spec = harness.spec(spec_id, "1").await.expect("seeded spec");

    let unknown_spec = AgentSpecRow {
        agent_spec_id: AgentSpecId::new(harness.ids.as_ref()),
        version: "9".to_owned(),
        digest: "e5".repeat(32),
        body: Vec::new(),
        created_at_ms: SEED_MS,
    };
    let unknown_envelope = harness.envelope(
        CMD_CREATE_TASK_RUN,
        "unknown-spec",
        create_task_run_payload(
            TaskId::new(harness.ids.as_ref()),
            RunId::new(harness.ids.as_ref()),
            Some(session_id),
            "chat",
            b"c",
            Some(&unknown_spec),
            None,
            0,
        ),
    );
    let error = harness
        .coordinator
        .execute(unknown_envelope.clone())
        .await
        .expect_err("unknown spec is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.record(&unknown_envelope).await.is_none());

    let wrong_digest = AgentSpecRow {
        agent_spec_id: spec.agent_spec_id,
        version: spec.version.clone(),
        digest: "f6".repeat(32),
        body: spec.body.clone(),
        created_at_ms: spec.created_at_ms,
    };
    let mismatched_envelope = harness.envelope(
        CMD_CREATE_TASK_RUN,
        "digest-mismatch",
        create_task_run_payload(
            TaskId::new(harness.ids.as_ref()),
            RunId::new(harness.ids.as_ref()),
            Some(session_id),
            "chat",
            b"c",
            Some(&wrong_digest),
            None,
            0,
        ),
    );
    let error = harness
        .coordinator
        .execute(mismatched_envelope.clone())
        .await
        .expect_err("digest mismatch is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(harness.record(&mismatched_envelope).await.is_none());
    let events = harness.events().await;
    assert!(events_of_type(&events, "RunCreated").is_empty());
    assert!(events_of_type(&events, "TaskCreated").is_empty());
}

#[tokio::test]
async fn failed_child_creation_stages_no_events() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let task_id = TaskId::new(harness.ids.as_ref());
    let parent = RunId::new(harness.ids.as_ref());
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "parent",
            create_task_run_payload(
                task_id,
                parent,
                Some(session_id),
                "chat",
                b"p",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("parent creation succeeds");
    let before = harness.events().await;

    let error = harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "bad-child",
            create_task_run_payload(
                TaskId::new(harness.ids.as_ref()),
                RunId::new(harness.ids.as_ref()),
                Some(session_id),
                "chat",
                b"c",
                None,
                Some(RunId::new(harness.ids.as_ref())),
                0,
            ),
        ))
        .await
        .expect_err("absent parent rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);

    assert_eq!(
        harness.events().await.len(),
        before.len(),
        "rejection stages no outbox events"
    );
}

#[tokio::test]
async fn malformed_payload_is_rejected_without_echoing_bytes() {
    let harness = Harness::new().await;
    let mut payload = vec![0xFF];
    payload.extend_from_slice(b"do-not-echo-payload-marker-7c1f");
    let envelope = harness.envelope(CMD_CREATE_TASK_RUN, "malformed", payload);

    let error = harness
        .coordinator
        .execute(envelope.clone())
        .await
        .expect_err("malformed payload rejected");

    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.message().contains("do-not-echo"),
        "rejection must not echo payload bytes: {}",
        error.message()
    );
    assert!(harness.record(&envelope).await.is_none());
    assert!(harness.events().await.is_empty());
}

#[tokio::test]
async fn event_ids_are_unique_across_a_multi_event_command() {
    let harness = Harness::new().await;
    let session_id = SessionId::new(harness.ids.as_ref());
    harness.seed_session(session_id).await;
    let task_id = TaskId::new(harness.ids.as_ref());
    let parent = RunId::new(harness.ids.as_ref());
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "parent",
            create_task_run_payload(
                task_id,
                parent,
                Some(session_id),
                "chat",
                b"p",
                None,
                None,
                0,
            ),
        ))
        .await
        .expect("parent creation succeeds");
    harness
        .coordinator
        .execute(harness.envelope(
            CMD_CREATE_TASK_RUN,
            "child",
            create_task_run_payload(
                task_id,
                RunId::new(harness.ids.as_ref()),
                Some(session_id),
                "chat",
                b"c",
                None,
                Some(parent),
                0,
            ),
        ))
        .await
        .expect("child creation succeeds");

    let events = harness.events().await;
    let mut ids: Vec<EventId> = events.iter().map(|event| event.event_id).collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "outbox event ids are unique");
    assert_eq!(total, 4, "two runs plus task and child events");
}
