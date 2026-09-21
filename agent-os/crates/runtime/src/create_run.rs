//! `CreateTaskRun`: task, graph head, and `Created` run in one transaction.

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{AgentSpecId, EventCursor, EventId, EventStreamKey, RunId, SessionId, TaskId};
use domain::provider::IdProvider;
use domain::run::{RecoveryDisposition, RunState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::{NewRun, NewTask};
use prost::Message;

use crate::{RuntimeDeps, decode_contract, optional_id, required_id, stage_catalogued};

/// Versioned agent spec reference validated before a run is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpecRef {
    /// Referenced agent spec.
    pub agent_spec_id: AgentSpecId,
    /// Referenced immutable version.
    pub version: String,
    /// Digest the caller expects the stored revision to carry.
    pub digest: String,
}

/// Typed mirror of the `CreateTaskRun` contract fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateTaskRunRequest {
    /// Task to create the run under.
    pub task_id: TaskId,
    /// Identifier of the run to create.
    pub run_id: RunId,
    /// Optional owning session.
    pub session_id: Option<SessionId>,
    /// Task kind persisted with a newly created task.
    pub task_kind: String,
    /// Task payload persisted with a newly created task.
    pub task_payload: Vec<u8>,
    /// Optional agent spec revision the run must reference.
    pub agent_spec_ref: Option<AgentSpecRef>,
    /// Optional parent run.
    pub parent_run_id: Option<RunId>,
    /// Cancellation epoch the caller observed on the parent run.
    pub observed_parent_cancellation_epoch: u64,
    /// Requested execution profile.
    pub requested_profile: String,
    /// Requested workspace URI.
    pub workspace_uri: String,
    /// Requested capabilities.
    pub requested_capabilities: Vec<String>,
    /// Requested budget.
    pub requested_budget: Vec<u8>,
}

impl CreateTaskRunRequest {
    /// Converts the decoded contract, drawing absent identifiers from `ids`.
    pub fn from_contract(
        raw: contract::CreateTaskRun,
        ids: &dyn IdProvider,
    ) -> errors::Result<Self> {
        let task_id = match optional_id::<TaskId>("task_id", &raw.task_id)? {
            Some(id) => id,
            None => TaskId::new(ids),
        };
        let run_id = match optional_id::<RunId>("run_id", &raw.run_id)? {
            Some(id) => id,
            None => RunId::new(ids),
        };
        let session_id = optional_id::<SessionId>("session_id", &raw.session_id)?;
        let parent_run_id = optional_id::<RunId>("parent_run_id", &raw.parent_run_id)?;
        let agent_spec_ref = match raw.agent_spec_ref {
            Some(reference) => {
                if reference.id.is_empty()
                    && reference.version.is_empty()
                    && reference.digest.is_empty()
                {
                    None
                } else {
                    Some(AgentSpecRef {
                        agent_spec_id: required_id::<AgentSpecId>(
                            "agent_spec_ref.id",
                            &reference.id,
                        )?,
                        version: reference.version,
                        digest: reference.digest,
                    })
                }
            }
            None => None,
        };
        Ok(Self {
            task_id,
            run_id,
            session_id,
            task_kind: raw.task_kind,
            task_payload: raw.task_payload,
            agent_spec_ref,
            parent_run_id,
            observed_parent_cancellation_epoch: raw.observed_parent_cancellation_epoch,
            requested_profile: raw.requested_profile,
            workspace_uri: raw.workspace_uri,
            requested_capabilities: raw.requested_capabilities,
            requested_budget: raw.requested_budget,
        })
    }
}

/// Creates the task (when new) with its graph head and the run in `Created`.
///
/// The referenced agent spec revision must be stored with the expected digest
/// and, when a parent is present, the observed parent cancellation epoch must
/// equal the persisted one; otherwise the creation fails before any row is
/// written. The run is never moved to `Ready` here.
pub async fn create_task_run(
    txn: &mut dyn KernelTxn,
    request: CreateTaskRunRequest,
    ctx: &CommandContext,
    now_ms: i64,
) -> errors::Result<RunId> {
    if let Some(reference) = &request.agent_spec_ref {
        match txn
            .agent_specs()
            .get(reference.agent_spec_id, &reference.version)
            .await?
        {
            Some(stored) if stored.digest == reference.digest => {}
            Some(_) => {
                return Err(failed_precondition(
                    "agent spec reference digest does not match the stored revision",
                ));
            }
            None => {
                return Err(failed_precondition("agent spec revision is not stored"));
            }
        }
    }
    let task_id = request.task_id;
    let session_id = request.session_id;
    let parent_run_id = request.parent_run_id;
    if let Some(parent_run_id) = parent_run_id {
        match txn.runs().get(parent_run_id).await? {
            Some(parent)
                if parent.cancellation_epoch == request.observed_parent_cancellation_epoch => {}
            Some(_) => {
                return Err(failed_precondition(
                    "observed parent cancellation epoch is stale",
                ));
            }
            None => {
                return Err(failed_precondition("parent run is not persisted"));
            }
        }
    }
    let run_id = request.run_id;
    let input_event_cursor = input_cursor(run_id)?;
    crate::task::ensure_task(
        txn,
        NewTask {
            task_id,
            session_id,
            created_by_actor_id: ctx.actor_id,
            task_kind: request.task_kind,
            payload: request.task_payload,
            created_at_ms: now_ms,
        },
    )
    .await?;
    txn.runs()
        .insert(NewRun {
            run_id,
            task_id,
            session_id,
            parent_run_id,
            state: RunState::Created,
            recovery: RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor,
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: request
                .agent_spec_ref
                .as_ref()
                .map(|reference| reference.agent_spec_id),
            agent_spec_version: request
                .agent_spec_ref
                .as_ref()
                .map(|reference| reference.version.clone()),
            agent_spec_digest: request
                .agent_spec_ref
                .as_ref()
                .map(|reference| reference.digest.clone()),
            requested_profile: request.requested_profile.clone(),
            workspace_uri: if request.workspace_uri.is_empty() {
                None
            } else {
                Some(request.workspace_uri.clone())
            },
            created_at_ms: now_ms,
        })
        .await?;
    Ok(run_id)
}

/// Handler for `agentos.spec.v1.CreateTaskRun`.
pub struct CreateTaskRunHandler {
    deps: RuntimeDeps,
}

impl CreateTaskRunHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for CreateTaskRunHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::CreateTaskRun>("CreateTaskRun", &payload)?;
        let request = CreateTaskRunRequest::from_contract(raw, self.deps.ids.as_ref())?;
        let now_ms = self.deps.now_unix_ms();
        let task_created = txn.tasks().get(request.task_id).await?.is_none();
        let run_id = create_task_run(txn, request.clone(), ctx, now_ms).await?;
        let cursor = input_cursor(run_id)?;
        if task_created {
            let task = contract::Task {
                task_id: request.task_id.to_string(),
                session_id: request
                    .session_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                created_by_actor_id: ctx.actor_id.to_string(),
                task_kind: request.task_kind.clone(),
                payload: request.task_payload.clone(),
                created_unix_ms: now_ms,
            };
            stage_catalogued(
                txn,
                EventId::new(self.deps.ids.as_ref()),
                "TaskCreated",
                StreamKey::task(request.task_id),
                task.encode_to_vec(),
                ctx.correlation_id.clone(),
                None,
            )
            .await?;
        }
        let run_event = run_created_event(&request, run_id, &cursor);
        stage_catalogued(
            txn,
            EventId::new(self.deps.ids.as_ref()),
            "RunCreated",
            StreamKey::run(run_id),
            run_event.clone(),
            ctx.correlation_id.clone(),
            None,
        )
        .await?;
        if let Some(parent_run_id) = request.parent_run_id {
            stage_catalogued(
                txn,
                EventId::new(self.deps.ids.as_ref()),
                "ChildRunCreated",
                StreamKey::run(parent_run_id),
                run_event,
                ctx.correlation_id.clone(),
                None,
            )
            .await?;
        }
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: run_id.to_string().into_bytes(),
        })
    }
}

fn run_created_event(
    request: &CreateTaskRunRequest,
    run_id: RunId,
    cursor: &EventCursor,
) -> Vec<u8> {
    contract::AgentRun {
        run_id: run_id.to_string(),
        task_id: request.task_id.to_string(),
        session_id: request
            .session_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        parent_run_id: request
            .parent_run_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        state: RunState::Created.to_wire(),
        recovery: RecoveryDisposition::Normal.to_wire(),
        run_revision: 0,
        loop_epoch: 0,
        step_sequence: 0,
        input_event_cursor: cursor.to_string(),
        cancellation_epoch: 0,
        resolved_environment_id: String::new(),
        output_ref: String::new(),
        current_turn_id: String::new(),
    }
    .encode_to_vec()
}

pub(crate) fn input_cursor(run_id: RunId) -> errors::Result<EventCursor> {
    EventStreamKey::new(format!("run/{run_id}"))
        .map(|key| EventCursor::new(key, 0))
        .map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "run stream key is not canonical",
            )
        })
}

fn failed_precondition(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, detail)
}
