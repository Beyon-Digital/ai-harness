//! Run driver worker (INT-001): advances every non-terminal run through
//! `Created -> Ready -> Running -> terminal` against the durable store,
//! driving one fenced loop turn at a time over the bound loop adapter's
//! supervised process.
//!
//! One `tick` performs each step through the command coordinator (or, for
//! claim/environment planning, inside its own write transaction), so every
//! transition stays auditable and replay-safe across daemon restart:
//!
//! - `Created`  — resolve `agent_loop` against the registry, plan the
//!   environment, submit `BindRun`.
//! - `Ready`    — submit `ClaimReadyRun` (CAS; expiry lets a later epoch
//!   reclaim).
//! - `Running`  — spawn/rebind the frozen loop bundle, then issue a turn
//!   and accept the decision; instructions route the follow-up work
//!   (`SpawnAgent` child request, `Wait` timer, `RequestApproval` draft)
//!   through the coordinator.
//! - `WaitingTool`/`WaitingChild` — re-drive once the wait clears (timer
//!   fired, every child terminal).
//! - `Running` runs left `RECOVERING` by a dead epoch are reclaimed under
//!   this epoch with a bumped `loop_epoch` before any new turn issues.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::workers::loops;
use adapter_protocol::framing::{read_frame, write_frame};
use adapter_protocol::handshake::ExpectedIdentity;
use adapter_protocol::session::SessionPhase;
use adapter_registry::SandboxTier;
use adapter_registry::registry::verify_for_spawn;
use adapter_registry::resolver::{Candidate, PortRequirement, resolve};
use command_coordinator::CommandCoordinator;
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{
    ActorId, AdapterId, AdapterInstanceId, CommandId, DaemonInstanceId, IdempotencyKey,
    PrincipalId, RunId,
};
use domain::provider::IdProvider;
use domain::run::{RecoveryDisposition, RunState};
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{RunCas, RunPatch};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use process_supervisor::spawn::{self, Child, SpawnSpec, terminate};
use prost::Message;
use runtime::create_run::AgentSpecRef;
use runtime::decision::{AcceptOutcome, DecisionInstruction};
use runtime::resolved_environment::EnvironmentPlan;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval};

use crate::workers::outbox::EpochSource;

/// Port id every fixture loop bundle implements.
pub const LOOP_PORT_ID: &str = "agent_loop";
/// Internal command type a loop `Wait` timer fires into.
pub const CMD_RUN_WAIT_EXPIRED: &str = "agentos.internal.RunWaitExpired";

const CLAIM_TTL_MS: u64 = 30_000;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Adapter bundle a run's frozen environment can spawn from.
#[derive(Clone, Debug)]
pub struct AdapterBundle {
    /// Registered adapter id.
    pub adapter_id: String,
    /// Registered version.
    pub version: String,
    /// Bundle root on disk (manifest + lock + entrypoint).
    pub dir: PathBuf,
}

/// Shared dependencies of the run driver.
pub struct RunWorkerDeps {
    /// Durable store.
    pub store: Arc<dyn KernelStore>,
    /// Envelope dispatch path for internal commands.
    pub coordinator: Arc<CommandCoordinator>,
    /// Daemon fencing epoch this worker runs under.
    pub epoch: Arc<dyn EpochSource>,
    /// Identifier source for internal commands.
    pub ids: Arc<dyn IdProvider>,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Daemon instance this worker belongs to.
    pub daemon_instance: DaemonInstanceId,
    /// Internal principal for worker commands.
    pub principal: PrincipalId,
    /// Internal actor for worker commands.
    pub actor: ActorId,
    /// Registered adapter bundle directories available for spawning.
    pub bundles: Vec<AdapterBundle>,
    /// Per-run loop script environment (`FIXTURE_LOOP_SCRIPT`).
    pub loop_scripts: Arc<HashMap<RunId, String>>,
}

struct LoopHandle {
    child: Child,
    phase: SessionPhase,
}

/// Interval loop that claims and drives non-terminal runs.
pub struct RunWorker {
    deps: Arc<RunWorkerDeps>,
    poll: Duration,
    loops: HashMap<RunId, LoopHandle>,
}

impl RunWorker {
    /// Creates a driver polling every `poll`.
    pub fn new(deps: RunWorkerDeps, poll: Duration) -> Self {
        Self {
            deps: Arc::new(deps),
            poll,
            loops: HashMap::new(),
        }
    }

    /// Drives runs until `shutdown` requests a stop; live loop children
    /// are terminated before returning.
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        let mut ticker = interval(self.poll);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
            }
            if let Err(error) = self.tick().await {
                tracing::warn!(error = %error, "run driver tick failed");
            }
        }
        let children = std::mem::take(&mut self.loops);
        for (run, handle) in children {
            let reason = terminate(handle.child, Duration::from_secs(2)).await;
            tracing::info!(run = %run_id_str(run), %reason, "loop child terminated at shutdown");
        }
    }

    async fn tick(&mut self) -> errors::Result<()> {
        let active = {
            let mut txn = self.read_txn().await?;
            txn.runs().list_active().await?
        };
        for row in active {
            if let Err(error) = self.step(&row).await {
                tracing::warn!(run = %row.run_id, error = %error, "run step failed");
            }
        }
        Ok(())
    }

    async fn step(&mut self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        match row.state {
            RunState::Created => self.bind(row).await,
            RunState::Ready => self.claim(row).await,
            RunState::Running => {
                if row.recovery == RecoveryDisposition::Recovering {
                    self.reclaim(row).await
                } else {
                    self.drive(row).await
                }
            }
            RunState::WaitingTool => {
                if self.wait_expired(row).await? {
                    self.drive(row).await
                } else {
                    Ok(())
                }
            }
            RunState::WaitingChild => {
                if self.children_terminal(row).await? {
                    self.drive(row).await
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    /// `Created -> Ready` via the `BindRun` internal command.
    async fn bind(&self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        let plan = {
            let mut txn = self.write_txn().await?;
            let plan = self.plan(&mut *txn, row).await;
            txn.rollback().await.ok();
            plan?
        };
        let payload = contract::BindRun {
            run_id: row.run_id.to_string(),
            expected_run_revision: row.run_revision,
            config_generation_id: plan.generation_id.to_string(),
            resolved_environment: Some(plan_to_contract(&plan)),
        }
        .encode_to_vec();
        self.submit(
            runtime::CMD_BIND_RUN,
            payload,
            format!("bind.{}", row.run_id),
        )
        .await
    }

    async fn plan(
        &self,
        txn: &mut dyn KernelTxn,
        row: &kernel_store::models::RunRow,
    ) -> errors::Result<EnvironmentPlan> {
        let candidates: Vec<Candidate> = txn
            .adapters()
            .list_registrations()
            .await?
            .into_iter()
            .map(Candidate::decode)
            .collect::<errors::Result<Vec<_>>>()?;
        let agent_loop = resolve(
            &PortRequirement {
                port_id: LOOP_PORT_ID.to_owned(),
                port_version: 1,
                required_capabilities: Vec::new(),
                sandbox_tier: SandboxTier::T0,
                pin_adapter_id: None,
                require_conformance_passed: false,
            },
            &candidates,
            &[],
        )?;
        let spec = match (
            row.agent_spec_id,
            row.agent_spec_version.as_deref(),
            row.agent_spec_digest.as_deref(),
        ) {
            (Some(id), Some(version), Some(digest)) => AgentSpecRef {
                agent_spec_id: id,
                version: version.to_owned(),
                digest: digest.to_owned(),
            },
            _ => {
                return Err(worker_error(
                    ErrorCode::FailedPrecondition,
                    "run has no agent spec binding input",
                ));
            }
        };
        runtime::resolved_environment::plan_environment(
            txn,
            self.deps.ids.as_ref(),
            row.run_id,
            &row.requested_profile,
            &spec,
            &agent_loop,
            row.workspace_uri.clone(),
            None,
            domain::resource::WorkspaceAccessMode::ReadOnly,
            Vec::new(),
            Vec::new(),
            self.deps.clock.now_unix_ms(),
        )
        .await
    }

    /// `Ready -> Running` via `ClaimReadyRun`.
    async fn claim(&self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        let payload = contract::ClaimReadyRun {
            run_id: row.run_id.to_string(),
            claim_owner: format!("run-worker:{}", self.deps.actor),
            claim_ttl_ms: CLAIM_TTL_MS,
        }
        .encode_to_vec();
        self.submit(
            runtime::CMD_CLAIM_READY_RUN,
            payload,
            format!("claim.{}", row.run_id),
        )
        .await
    }

    /// Reclaims a `Running` run left `RECOVERING` by a dead epoch, bumping
    /// `loop_epoch` so decisions issued to the previous process go stale.
    async fn reclaim(&mut self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        self.loops.remove(&row.run_id);
        let now = self.deps.clock.now_unix_ms();
        let mut txn = self.write_txn().await?;
        let moved = txn
            .runs()
            .cas_update(
                row.run_id,
                RunCas {
                    run_revision: row.run_revision,
                    state: Some(RunState::Running),
                    cancellation_epoch: None,
                },
                RunPatch {
                    recovery: Some(RecoveryDisposition::Normal),
                    loop_epoch: Some(row.loop_epoch + 1),
                    claim: Some(kernel_store::models::ClaimPatch {
                        owner: format!("run-worker:{}", self.deps.actor),
                        token: fresh_token(self.deps.ids.as_ref()),
                        expires_unix_ms: now.saturating_add(CLAIM_TTL_MS as i64),
                        daemon_epoch: self.deps.epoch.epoch(),
                    }),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await?;
        if moved {
            txn.commit().await?;
        } else {
            txn.rollback().await.ok();
        }
        Ok(())
    }

    /// Issues one turn against the run's live loop process and accepts the
    /// decision; handles the follow-up instruction.
    async fn drive(&mut self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        if !self.claim_ours(row) {
            return Ok(());
        }
        if !self.loops.contains_key(&row.run_id) {
            let handle = self.spawn_loop(row).await?;
            self.loops.insert(row.run_id, handle);
        }
        let script = self.deps.loop_scripts.get(&row.run_id).cloned();
        let _ = script; // scripts are passed through the spawn env, not per turn.
        let handle = self.loops.get_mut(&row.run_id).expect("loop handle");
        let ctx = TxContext {
            daemon_epoch: self.deps.epoch.epoch(),
            principal_id: self.deps.principal,
            command_id: CommandId::new(self.deps.ids.as_ref()),
            correlation_id: None,
        };
        let outcome = loops::drive_turn(
            self.deps.store.as_ref(),
            self.deps.ids.as_ref(),
            &ctx,
            &mut handle.child,
            &mut handle.phase,
            row.run_id,
            Vec::new(),
            Vec::new(),
            CALL_TIMEOUT,
            self.deps.clock.now_unix_ms(),
        )
        .await?;
        let AcceptOutcome::Accepted { instruction, .. } = outcome else {
            return Ok(());
        };
        self.follow_up(row, instruction).await
    }

    /// Routes the durable instruction from an accepted decision.
    async fn follow_up(
        &mut self,
        row: &kernel_store::models::RunRow,
        instruction: DecisionInstruction,
    ) -> errors::Result<()> {
        match instruction {
            DecisionInstruction::Complete { .. } | DecisionInstruction::Fail { .. } => {
                if let Some(handle) = self.loops.remove(&row.run_id) {
                    let _ = terminate(handle.child, Duration::from_secs(2)).await;
                }
                Ok(())
            }
            DecisionInstruction::SpawnAgent { child_request } => {
                self.submit(
                    runtime::CMD_CREATE_TASK_RUN,
                    child_request,
                    format!("spawn.{}.{}", row.run_id, row.step_sequence),
                )
                .await
            }
            DecisionInstruction::Wait { timer_id, .. } => {
                let timer_id = if timer_id.is_empty() {
                    self.deps.ids.new_uuid_v7().to_string()
                } else {
                    timer_id
                };
                let payload = contract::ScheduleTimer {
                    timer_id: timer_id.clone(),
                    run_id: row.run_id.to_string(),
                    timer_kind: CMD_RUN_WAIT_EXPIRED.to_owned(),
                    due_at_ms: self.deps.clock.now_unix_ms(),
                    payload: row.run_id.to_string().into_bytes(),
                }
                .encode_to_vec();
                self.submit(
                    runtime::CMD_SCHEDULE_TIMER,
                    payload,
                    format!("wait.{}.{}", row.run_id, timer_id),
                )
                .await
            }
            DecisionInstruction::RequestApproval { approval_draft } => {
                self.submit(
                    approvals::CMD_CREATE_APPROVAL_REQUEST,
                    approval_draft,
                    format!("approval.{}.{}", row.run_id, row.step_sequence),
                )
                .await
            }
            DecisionInstruction::InvokeEffect { .. } => Err(worker_error(
                ErrorCode::FailedPrecondition,
                "effect invocation is not wired in this driver yet",
            )),
        }
    }

    /// True when the run's `Wait` timer has fired.
    async fn wait_expired(&self, row: &kernel_store::models::RunRow) -> errors::Result<bool> {
        let mut txn = self.read_txn().await?;
        let timers = txn.timers().list_by_run(row.run_id).await?;
        Ok(timers
            .iter()
            .any(|timer| timer.state == domain::resource::TimerState::Fired))
    }

    /// True when every child run of `row` is terminal.
    async fn children_terminal(&self, row: &kernel_store::models::RunRow) -> errors::Result<bool> {
        let mut txn = self.read_txn().await?;
        let all = txn.runs().list_by_task(row.task_id).await?;
        let children_done = all
            .iter()
            .filter(|child| child.parent_run_id == Some(row.run_id))
            .all(|child| child.state.is_terminal());
        let has_children = all
            .iter()
            .any(|child| child.parent_run_id == Some(row.run_id));
        Ok(has_children && children_done)
    }

    /// Claims a run only when the live claim belongs to this worker epoch.
    fn claim_ours(&self, row: &kernel_store::models::RunRow) -> bool {
        row.claim_daemon_epoch == Some(self.deps.epoch.epoch())
            && row
                .claim_owner
                .as_deref()
                .is_some_and(|owner| owner == format!("run-worker:{}", self.deps.actor))
    }

    /// Spawns the frozen loop bundle for `row` and completes the adapter
    /// handshake on the private socketpair.
    async fn spawn_loop(&self, row: &kernel_store::models::RunRow) -> errors::Result<LoopHandle> {
        let (adapter_id, version, digest, script) = {
            let mut txn = self.read_txn().await?;
            let environment_id = row
                .resolved_environment_id
                .ok_or_else(|| worker_error(ErrorCode::FailedPrecondition, "run is not bound"))?;
            let environment = txn
                .environments()
                .get_environment(environment_id)
                .await?
                .ok_or_else(|| worker_error(ErrorCode::NotFound, "run environment missing"))?;
            let script = self
                .deps
                .loop_scripts
                .get(&row.run_id)
                .cloned()
                .unwrap_or_default();
            (
                environment.agent_loop_id,
                environment.agent_loop_version,
                environment.agent_loop_digest,
                script,
            )
        };
        let adapter_uuid = AdapterId::from_str(&adapter_id).map_err(|_| {
            worker_error(
                ErrorCode::Internal,
                "bound agent_loop id is not an adapter id",
            )
        })?;
        let bundle = self
            .deps
            .bundles
            .iter()
            .find(|bundle| bundle.adapter_id == adapter_id && bundle.version == version)
            .cloned()
            .ok_or_else(|| {
                worker_error(
                    ErrorCode::FailedPrecondition,
                    "no local bundle for the frozen loop adapter",
                )
            })?;
        let verified = {
            let mut txn = self.write_txn().await?;
            let verified =
                verify_for_spawn(&mut *txn, adapter_uuid, &version, &digest, &bundle.dir).await;
            txn.rollback().await.ok();
            verified?
        };
        let adapter_instance = AdapterInstanceId::new(self.deps.ids.as_ref());
        let mut env = vec![
            ("FIXTURE_LOOP_ADAPTER_ID".to_owned(), adapter_id.clone()),
            ("FIXTURE_LOOP_ADAPTER_VERSION".to_owned(), version.clone()),
            ("FIXTURE_LOOP_SCRIPT".to_owned(), script),
        ];
        if let Ok(level) = std::env::var("RUST_LOG") {
            env.push(("RUST_LOG".to_owned(), level));
        }
        let mut child = spawn::spawn(&SpawnSpec {
            adapter_id: adapter_uuid,
            adapter_version: version.clone(),
            expected_bundle_digest: digest.clone(),
            adapter_instance_id: adapter_instance,
            daemon_instance_id: self.deps.daemon_instance,
            daemon_fencing_epoch: self.deps.epoch.epoch(),
            protocol_version: 1,
            executable: verified.entrypoint,
            argv: Vec::new(),
            env,
            cwd: None,
        })?;

        // Adapter handshake: kernel writes Bootstrap, child replies Hello.
        let nonce = self.deps.ids.new_uuid_v7().to_string();
        let identity = ExpectedIdentity {
            daemon_instance_id: self.deps.daemon_instance,
            daemon_fencing_epoch: self.deps.epoch.epoch(),
            adapter_instance_id: adapter_instance.to_string(),
            adapter_id: adapter_id.clone(),
            adapter_version: version,
            expected_bundle_digest: digest,
            protocol_version: 1,
        };
        let stream = child.ipc();
        write_frame(stream, &identity.bootstrap_frame(&nonce))?;
        let hello_deadline = Instant::now() + CALL_TIMEOUT;
        stream
            .set_read_timeout(Some(
                hello_deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_default(),
            ))
            .map_err(|error| {
                worker_error(ErrorCode::Unavailable, "loop ipc timeout setup failed")
                    .with_source(error)
            })?;
        let phase = match read_frame(stream)? {
            Some(contract::AdapterFrame {
                body: Some(contract::adapter_frame::Body::Hello(hello)),
            }) => {
                identity.verify_hello(&hello, &nonce)?;
                SessionPhase::Ready
            }
            _ => {
                return Err(worker_error(
                    ErrorCode::Unavailable,
                    "loop adapter did not complete the handshake",
                ));
            }
        };
        Ok(LoopHandle { child, phase })
    }

    /// Submits an internal command through the coordinator.
    async fn submit(
        &self,
        command_type: &str,
        payload: Vec<u8>,
        idempotency_key: String,
    ) -> errors::Result<()> {
        let digest_hex: String = Sha256::digest(&payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.deps
            .coordinator
            .execute(CommandEnvelope {
                command_id: CommandId::new(self.deps.ids.as_ref()),
                idempotency_key: IdempotencyKey::new(idempotency_key).map_err(|_| {
                    worker_error(
                        ErrorCode::InvalidArgument,
                        "derived idempotency key malformed",
                    )
                })?,
                principal_id: self.deps.principal,
                actor_id: self.deps.actor,
                device_id: None,
                delegation_chain_id: None,
                request_digest: RequestDigest::from_str(&digest_hex)
                    .expect("sha256 hex is a valid request digest"),
                correlation_id: None,
                causation_id: None,
                deadline_unix_ms: None,
                command_type: command_type.to_owned(),
                payload,
            })
            .await?;
        Ok(())
    }

    fn tx_context(&self) -> TxContext {
        TxContext {
            daemon_epoch: self.deps.epoch.epoch(),
            principal_id: self.deps.principal,
            command_id: CommandId::new(self.deps.ids.as_ref()),
            correlation_id: None,
        }
    }

    async fn read_txn(&self) -> errors::Result<Box<dyn kernel_store::KernelReadTxn + '_>> {
        self.deps.store.begin_read().await
    }

    async fn write_txn(&self) -> errors::Result<Box<dyn KernelTxn + '_>> {
        self.deps.store.begin_write(self.tx_context()).await
    }
}

/// Handler for `agentos.internal.RunWaitExpired` — a fired loop `Wait`
/// timer resumes its run (`WaitingTool -> Running`) so the driver issues
/// the next turn. Internal worker command; not part of the public catalog.
pub struct RunWaitExpiredHandler;

#[async_trait::async_trait]
impl CommandHandler for RunWaitExpiredHandler {
    async fn handle(
        &self,
        _ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let text = String::from_utf8(payload).map_err(|_| {
            worker_error(
                ErrorCode::InvalidArgument,
                "wait-expired payload is not utf-8",
            )
        })?;
        let run_id = RunId::from_str(&text).map_err(|_| {
            worker_error(
                ErrorCode::InvalidArgument,
                "wait-expired payload is not a run id",
            )
        })?;
        let run = txn
            .runs()
            .get(run_id)
            .await?
            .ok_or_else(|| worker_error(ErrorCode::NotFound, "run not found"))?;
        if run.state == RunState::WaitingTool {
            txn.runs()
                .cas_update(
                    run_id,
                    RunCas {
                        run_revision: run.run_revision,
                        state: Some(RunState::WaitingTool),
                        cancellation_epoch: None,
                    },
                    RunPatch {
                        state: Some(RunState::Running),
                        bump_revision: true,
                        ..RunPatch::default()
                    },
                )
                .await?;
        }
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: Vec::new(),
        })
    }
}

fn run_id_str(run: RunId) -> String {
    run.to_string()
}

fn fresh_token(ids: &dyn IdProvider) -> u64 {
    (ids.new_uuid_v7().as_u128() & i64::MAX as u128) as u64
}

fn worker_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Converts a planned environment into the `BindRun` contract payload.
fn plan_to_contract(plan: &EnvironmentPlan) -> contract::ResolvedRunEnvironment {
    let environment = &plan.environment;
    contract::ResolvedRunEnvironment {
        id: environment.environment_id.to_string(),
        run_id: environment.run_id.to_string(),
        agent_spec: Some(contract::VersionedRef {
            id: environment.agent_spec_id.to_string(),
            version: environment.agent_spec_version.clone(),
            digest: environment.agent_spec_digest.clone(),
        }),
        agent_loop: Some(contract::VersionedRef {
            id: environment.agent_loop_id.clone(),
            version: environment.agent_loop_version.clone(),
            digest: environment.agent_loop_digest.clone(),
        }),
        config_generation_id: environment.config_generation_id.to_string(),
        bindings: plan
            .bindings
            .iter()
            .map(|binding| contract::ResolvedBinding {
                port_id: binding.port_id.clone(),
                adapter: Some(contract::VersionedRef {
                    id: binding.adapter_id.to_string(),
                    version: binding.adapter_version.clone(),
                    digest: binding.adapter_digest.clone(),
                }),
                capabilities: serde_json::from_slice(&binding.capabilities).unwrap_or_default(),
            })
            .collect(),
        workspace_uri: environment
            .workspace_uri
            .as_ref()
            .map(|uri| contract::ResourceUri { uri: uri.clone() }),
        workspace_base_revision: environment
            .workspace_base_revision
            .clone()
            .unwrap_or_default(),
        workspace_mode: environment.workspace_mode.to_wire(),
        model_provider: environment.model_provider.clone().unwrap_or_default(),
        model_id: environment.model_id.clone().unwrap_or_default(),
        model_parameters_json: environment.model_parameters.clone().unwrap_or_default(),
        kernel_version: environment.kernel_version.clone(),
        protocol_versions: serde_json::from_slice::<Vec<u32>>(&environment.protocol_versions)
            .unwrap_or_default()
            .iter()
            .map(u32::to_string)
            .collect(),
        capability_grant_ids: String::from_utf8_lossy(&environment.capability_grant_ids)
            .split('\n')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect(),
        approval_request_ids: String::from_utf8_lossy(&environment.approval_request_ids)
            .split('\n')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect(),
    }
}
