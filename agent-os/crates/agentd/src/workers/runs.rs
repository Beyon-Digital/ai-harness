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
use adapter_protocol::session::{self, SessionPhase};
use adapter_registry::SandboxTier;
use adapter_registry::registry::verify_for_spawn;
use adapter_registry::resolver::{Candidate, PortRequirement, resolve};
use command_coordinator::CommandCoordinator;
use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::effect::EffectState;
use domain::generated::contract;
use domain::ids::{
    ActorId, AdapterId, AdapterInstanceId, CommandId, DaemonInstanceId, EnvironmentId,
    IdempotencyKey, PrincipalId, RunId,
};
use domain::provider::IdProvider;
use domain::run::{RecoveryDisposition, RunState};
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{EffectRow, RunCas, RunPatch};
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
/// Port id effect invocations dispatch against.
pub const EFFECT_PORT_ID: &str = "effect.execute";
/// Internal command type a loop `Wait` timer fires into.
pub const CMD_RUN_WAIT_EXPIRED: &str = "agentos.internal.RunWaitExpired";

/// Internal command that terminalizes a `Cancelling` run once its in-flight
/// adapter work has been stopped (worker-owned; not in the public catalog).
pub const CMD_RUN_CANCEL_COMPLETE: &str = "agentos.internal.RunCancelComplete";

const CLAIM_TTL_MS: u64 = 30_000;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Loop `next` calls may wait on a model round-trip; keep the deadline
/// comfortably above the adapters' HTTP timeout (default 55s).
const LOOP_CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Model-loop approvals expire this long after the daemon stamps them.
const APPROVAL_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Adapter bundle a run's frozen environment can spawn from.
#[derive(Clone, Debug)]
pub struct AdapterBundle {
    /// Registered adapter id.
    pub adapter_id: String,
    /// Registered version.
    pub version: String,
    /// Bundle root on disk (manifest + lock + entrypoint).
    pub dir: PathBuf,
    /// `sha256:` bundle digest — spawn lookup matches the full
    /// `(id, version, digest)` identity so same-version bundles from
    /// different dirs never collide.
    pub bundle_digest: String,
    /// Kernel isolation the bundle's manifest requests at spawn.
    pub isolation: spawn::Isolation,
    /// Manifest `runtime.type` — `process` (entrypoint is an executable)
    /// or `wasm` (entrypoint is a `.wasm` module run by the wasm host).
    pub runtime_type: String,
}

/// Resolves what actually spawns for a bundle: a `wasm` bundle's module
/// runs inside `agentos-wasm-host` with WASI stdio mapped onto the IPC
/// socketpair (`stdout_ipc`); a `process` bundle execs its entrypoint.
fn spawn_target(
    bundle: &AdapterBundle,
    entrypoint: std::path::PathBuf,
) -> errors::Result<(std::path::PathBuf, Vec<String>, bool)> {
    if bundle.runtime_type == "wasm" {
        let host = adapter_registry::wasm_host_binary().ok_or_else(|| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "wasm bundle requires the agentos-wasm-host binary (set AGENTOS_WASM_HOST)",
            )
        })?;
        Ok((host, vec![entrypoint.to_string_lossy().into_owned()], true))
    } else {
        Ok((entrypoint, Vec::new(), false))
    }
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
    /// Extra environment injected into spawned loop-adapter processes
    /// (e.g. `FIXTURE_LOOP_RECORD_DIR` in tests).
    pub loop_env: HashMap<String, String>,
    /// Daemon runtime dir — hosts the fixture adapter's durable store so
    /// provider state survives a daemon restart.
    pub runtime_dir: PathBuf,
    /// Extra environment injected into spawned effect-adapter processes
    /// (fixture fault flags like `FIXTURE_CRASH_BEFORE_RESPONSE`).
    pub effect_env: HashMap<String, String>,
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
    /// Per-run consecutive-failure count and earliest retry time — a run
    /// whose step keeps failing (e.g. a queued run missing its spec
    /// binding) backs off instead of spinning every poll.
    retry_after: HashMap<RunId, (u32, Instant)>,
    /// `limits.context` per resolved environment — the generation doc is
    /// immutable once activated, so one parse per env is enough.
    context_limits: HashMap<EnvironmentId, config_engine::schema::ContextLimits>,
}

impl RunWorker {
    /// Creates a driver polling every `poll`.
    pub fn new(deps: RunWorkerDeps, poll: Duration) -> Self {
        Self {
            deps: Arc::new(deps),
            poll,
            loops: HashMap::new(),
            retry_after: HashMap::new(),
            context_limits: HashMap::new(),
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
            let instance = handle.child.instance;
            let reason = terminate(handle.child, Duration::from_secs(2)).await;
            self.mark_instance_exit(instance, "exited", reason.to_string())
                .await;
            tracing::info!(run = %run_id_str(run), %reason, "loop child terminated at shutdown");
        }
    }

    async fn tick(&mut self) -> errors::Result<()> {
        let active = {
            let mut txn = self.read_txn().await?;
            txn.runs().list_active().await?
        };
        let active_ids: std::collections::HashSet<RunId> =
            active.iter().map(|r| r.run_id).collect();
        self.retry_after.retain(|id, _| active_ids.contains(id));
        let now = Instant::now();
        for row in active {
            if self
                .retry_after
                .get(&row.run_id)
                .is_some_and(|(_, next)| *next > now)
            {
                continue;
            }
            match self.step(&row).await {
                Ok(()) => {
                    self.retry_after.remove(&row.run_id);
                }
                Err(error) => {
                    let fails = self
                        .retry_after
                        .get(&row.run_id)
                        .map(|(n, _)| n.saturating_add(1))
                        .unwrap_or(1);
                    // Exponential backoff: poll * 2^(fails-1), capped at 5s.
                    let backoff = self
                        .poll
                        .saturating_mul(1u32 << fails.saturating_sub(1).min(7))
                        .min(Duration::from_secs(5));
                    self.retry_after
                        .insert(row.run_id, (fails, Instant::now() + backoff));
                    tracing::warn!(run = %row.run_id, %fails, error = %error, "run step failed");
                }
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
                if self.claim_ours(row) {
                    self.step_waiting_tool(row).await
                } else {
                    self.restake_claim(row).await
                }
            }
            RunState::WaitingChild => {
                if !self.claim_ours(row) {
                    self.restake_claim(row).await
                } else if self.children_terminal(row).await? {
                    self.drive(row).await
                } else {
                    Ok(())
                }
            }
            RunState::WaitingHuman => {
                if !self.claim_ours(row) {
                    self.restake_claim(row).await
                } else {
                    self.step_waiting_human(row).await
                }
            }
            // The in-flight work for this run is the loop child this epoch
            // owns — terminate it, then terminalize through the internal
            // command so the event + revision stay auditable.
            RunState::Cancelling => self.finalize_cancel(row).await,
            _ => Ok(()),
        }
    }

    /// `WaitingHuman`: resume once the run's latest approval resolves —
    /// approved returns to `Running` so the next turn issues; denied fails
    /// the run. Pending, expired, or invalidated requests leave the run
    /// parked (renew, respond, or cancel is the operator's move; recovery
    /// reports an expired request as `RequiresHumanDecision`).
    async fn step_waiting_human(
        &mut self,
        row: &kernel_store::models::RunRow,
    ) -> errors::Result<()> {
        let now = self.deps.clock.now_unix_ms();
        let mut txn = self.write_txn().await?;
        let latest = {
            let approvals = txn.security().list_approvals_by_run(row.run_id).await?;
            approvals
                .into_iter()
                .max_by_key(|a| (a.created_at_ms, a.request_id))
        };
        let Some(latest) = latest else {
            txn.rollback().await.ok();
            return Ok(());
        };
        let Ok(expected) = approvals::ApprovalDigest::from_str(&latest.request_digest) else {
            txn.rollback().await.ok();
            return Ok(());
        };
        let outcome = approvals::is_satisfied(&mut *txn, latest.request_id, &expected, now).await?;
        let (next, denied) = match outcome {
            approvals::ApprovalOutcome::Approved => (
                RunPatch {
                    state: Some(RunState::Running),
                    bump_revision: true,
                    ..RunPatch::default()
                },
                false,
            ),
            approvals::ApprovalOutcome::Denied => (
                RunPatch {
                    state: Some(RunState::Failed),
                    terminal_reason: Some("approval denied".to_owned()),
                    bump_revision: true,
                    ..RunPatch::default()
                },
                true,
            ),
            _ => {
                txn.rollback().await.ok();
                return Ok(());
            }
        };
        let moved = txn
            .runs()
            .cas_update(
                row.run_id,
                RunCas {
                    run_revision: row.run_revision,
                    state: Some(RunState::WaitingHuman),
                    cancellation_epoch: None,
                },
                next,
            )
            .await?;
        if moved {
            txn.commit().await?;
            if denied {
                self.stop_loop(row.run_id, "failed").await;
            }
        } else {
            txn.rollback().await.ok();
        }
        Ok(())
    }

    /// Persists the `adapter_instances` row every spawned process must have
    /// (specs/process-supervisor.md): `starting` at spawn, `ready` after a
    /// verified handshake, `exited`/`failed` when the process ends.
    async fn record_instance_spawn(
        &self,
        adapter_id: AdapterId,
        adapter_version: &str,
        bundle_digest: &str,
        instance: AdapterInstanceId,
        pid: u32,
        start_identity: &str,
    ) -> errors::Result<()> {
        let mut txn = self.write_txn().await?;
        txn.adapters()
            .insert_instance(kernel_store::models::NewAdapterInstance {
                adapter_instance_id: instance,
                adapter_id,
                adapter_version: adapter_version.to_owned(),
                bundle_digest: bundle_digest.to_owned(),
                daemon_instance_id: self.deps.daemon_instance,
                pid: Some(i64::from(pid)),
                process_start_identity: Some(start_identity.to_owned()),
                state: "starting".to_owned(),
                exit_reason: None,
                last_heartbeat_ms: None,
                started_at_ms: self.deps.clock.now_unix_ms(),
                ended_at_ms: None,
            })
            .await?;
        txn.commit().await
    }

    /// CAS `starting -> ready` once the adapter's hello verifies.
    async fn mark_instance_ready(&self, instance: AdapterInstanceId) -> errors::Result<()> {
        let mut txn = self.write_txn().await?;
        let moved = txn
            .adapters()
            .cas_instance_state(
                instance,
                "starting",
                kernel_store::models::AdapterInstanceStatePatch {
                    state: Some("ready".to_owned()),
                    exit_reason: None,
                    last_heartbeat_ms: Some(self.deps.clock.now_unix_ms()),
                    ended_at_ms: None,
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

    /// Best-effort terminal instance update — the row is observability, so
    /// store failures never block termination. Accepts `ready` or
    /// `starting` (a process can die before its hello completes).
    async fn mark_instance_exit(
        &self,
        instance: AdapterInstanceId,
        state: &'static str,
        reason: String,
    ) {
        let Ok(mut txn) = self.write_txn().await else {
            return;
        };
        let patch = || kernel_store::models::AdapterInstanceStatePatch {
            state: Some(state.to_owned()),
            exit_reason: Some(reason.clone()),
            last_heartbeat_ms: None,
            ended_at_ms: Some(self.deps.clock.now_unix_ms()),
        };
        let mut moved = txn
            .adapters()
            .cas_instance_state(instance, "ready", patch())
            .await
            .unwrap_or(false);
        if !moved {
            moved = txn
                .adapters()
                .cas_instance_state(instance, "starting", patch())
                .await
                .unwrap_or(false);
        }
        if moved {
            txn.commit().await.ok();
        } else {
            txn.rollback().await.ok();
        }
    }

    /// Removes the cached loop child, terminates it, and marks its
    /// `adapter_instances` row terminal.
    async fn stop_loop(&mut self, run_id: RunId, state: &'static str) {
        if let Some(handle) = self.loops.remove(&run_id) {
            let instance = handle.child.instance;
            let reason = terminate(handle.child, Duration::from_secs(2)).await;
            self.mark_instance_exit(instance, state, reason.to_string())
                .await;
        }
    }

    /// `Cancelling -> Cancelled`: terminate this epoch's loop child, then
    /// finalize through the coordinator so the terminal transition + events
    /// commit under the fencing epoch.
    async fn finalize_cancel(&mut self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        self.stop_loop(row.run_id, "exited").await;
        self.submit(
            CMD_RUN_CANCEL_COMPLETE,
            row.run_id.to_string().into_bytes(),
            format!("cancel-complete.{}", row.run_id),
        )
        .await?;
        Ok(())
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
        // Resolution may only pick adapters whose exact `(id, version,
        // digest)` identity has an on-disk bundle this daemon can spawn —
        // a disabled/removed installed adapter's durable registration
        // stays for audit but can never satisfy a new run's binding.
        let candidates: Vec<Candidate> = txn
            .adapters()
            .list_registrations()
            .await?
            .into_iter()
            .map(Candidate::decode)
            .collect::<errors::Result<Vec<_>>>()?
            .into_iter()
            .filter(|c| {
                self.deps.bundles.iter().any(|b| {
                    b.adapter_id == c.row.adapter_id.to_string()
                        && b.version == c.row.version
                        && b.bundle_digest == c.row.bundle_digest
                })
            })
            .collect();
        // Honor the profile's `agent_loop` binding: pin resolution to
        // that adapter id so a run bound to `acp-local` actually spawns
        // the acp-loop adapter instead of the first registered loop.
        // Builtin names (`fixture-loop`) have no adapter id — leave the
        // pin unset and keep the generic resolution.
        let loop_pin = self.profile_loop_pin(txn, &row.requested_profile).await?;
        let agent_loop = resolve(
            &PortRequirement {
                port_id: LOOP_PORT_ID.to_owned(),
                port_version: 1,
                required_capabilities: Vec::new(),
                sandbox_tier: SandboxTier::T0,
                pin_adapter_id: loop_pin,
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
        let local_bundles: Vec<(String, String, String)> = self
            .deps
            .bundles
            .iter()
            .map(|b| {
                (
                    b.adapter_id.clone(),
                    b.version.clone(),
                    b.bundle_digest.clone(),
                )
            })
            .collect();
        runtime::resolved_environment::plan_environment(
            txn,
            self.deps.ids.as_ref(),
            row.run_id,
            &row.requested_profile,
            &spec,
            &agent_loop,
            &local_bundles,
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
        // A cached handle whose epoch lost the claim must not keep running:
        // terminate it before the loop_epoch bump spawns a replacement.
        self.stop_loop(row.run_id, "failed").await;
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

    /// Re-stamps the run claim under this epoch without touching the state
    /// — a waiting run (`WaitingTool`/`WaitingChild`) keeps its wait reason
    /// while a dead epoch's claim is fenced off. The next tick then takes
    /// the normal `claim_ours` path.
    async fn restake_claim(&mut self, row: &kernel_store::models::RunRow) -> errors::Result<()> {
        let now = self.deps.clock.now_unix_ms();
        let mut txn = self.write_txn().await?;
        let moved = txn
            .runs()
            .cas_update(
                row.run_id,
                RunCas {
                    run_revision: row.run_revision,
                    state: Some(row.state),
                    cancellation_epoch: None,
                },
                RunPatch {
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
        let ctx = TxContext {
            daemon_epoch: self.deps.epoch.epoch(),
            principal_id: self.deps.principal,
            command_id: CommandId::new(self.deps.ids.as_ref()),
            correlation_id: None,
        };
        // The opaque run-state snapshot carries the task payload so real
        // loop adapters (LLM) can see what the run is doing; the
        // new-events batch carries effect outcomes settled by the last
        // accepted decision's step — a settled effect is eligible for
        // exactly the turn issued right after the decision that created
        // it, so a wait/approval resume cannot replay an old result.
        let limits = self.context_limits_for(row).await?;
        let (state, events) = {
            let mut txn = self.read_txn().await?;
            let mut state = txn
                .tasks()
                .get(row.task_id)
                .await?
                .map(|task| task.payload)
                .unwrap_or_default();
            // Context strategy (Phase 10): the profile's generation caps
            // how much a single turn's LoopInput may carry. State is
            // prompt text — truncate on a UTF-8 boundary; the events batch
            // keeps the most recent entries that fit the byte cap.
            let cap = limits.max_state_bytes as usize;
            if state.len() > cap {
                let mut end = cap.min(state.len());
                // Walk back past UTF-8 continuation bytes (0b10xxxxxx).
                while end > 0 && state[end] & 0xC0 == 0x80 {
                    end -= 1;
                }
                state.truncate(end);
                tracing::info!(run = %run_id_str(row.run_id), cap, "loop state truncated");
            }
            let run_effects = txn.effects().list_by_run(row.run_id).await?;
            // `settled` carries only the current step's terminal effects
            // (fed exactly once — replay is deliberate). `op_counts`
            // summarizes ALL terminal effects for the run so loops can
            // bound per-operation work (e.g. MCP tool hops) without the
            // feed re-delivering history. `events` stays the version-1
            // ARRAY of settled-effect objects; the counts ride in a
            // leading marker element (`kernel.op_counts`) that parses
            // like any other settled entry so v1 loops stay compatible.
            // At most 64 distinct operation names are counted — beyond
            // that the counts are advisory anyway.
            let mut op_counts = std::collections::BTreeMap::<String, u64>::new();
            let mut settled: Vec<serde_json::Value> = Vec::new();
            for e in &run_effects {
                if !effects::is_terminal(e.state) {
                    continue;
                }
                if op_counts.len() < 64 || op_counts.contains_key(&e.operation) {
                    *op_counts.entry(e.operation.clone()).or_default() += 1;
                }
                if e.step_sequence == row.step_sequence {
                    settled.push(serde_json::json!({
                        "effect_id": e.effect_id.to_string(),
                        "operation": e.operation,
                        "state": format!("{:?}", e.state).to_ascii_lowercase(),
                        "result_ref": e.result_ref,
                        "error_code": e.error_code,
                    }));
                }
            }
            let drop_n = settled.len().saturating_sub(limits.max_fed_events as usize);
            if drop_n > 0 {
                tracing::info!(run = %run_id_str(row.run_id), drop_n, "oldest fed events dropped");
                settled.drain(..drop_n);
            }
            let marker = |counts: &std::collections::BTreeMap<String, u64>| {
                serde_json::json!({
                    "effect_id": "",
                    "operation": "kernel.op_counts",
                    "state": "committed",
                    "result_ref": "",
                    "error_code": "",
                    "op_counts": counts,
                })
            };
            let pack = |settled: &[serde_json::Value], counts: Option<&serde_json::Value>| {
                let mut v = Vec::with_capacity(settled.len() + 1);
                if let Some(m) = counts {
                    v.push(m.clone());
                }
                v.extend_from_slice(settled);
                serde_json::to_vec(&v).unwrap_or_default()
            };
            let marker_value = marker(&op_counts);
            let mut events = pack(&settled, Some(&marker_value));
            // Shrink order: oldest settled entries first, then the
            // marker itself, so the newest result never silently drops.
            while events.len() > limits.max_fed_event_bytes as usize && !settled.is_empty() {
                settled.remove(0);
                events = pack(&settled, Some(&marker_value));
            }
            if events.len() > limits.max_fed_event_bytes as usize {
                events = pack(&settled, None);
            }
            // A cap too small even for the settled batch — emit the
            // empty payload (0 bytes) so the configured bound holds.
            if events.len() > limits.max_fed_event_bytes as usize {
                events = Vec::new();
            }
            (state, events)
        };
        let handle = self.loops.get_mut(&row.run_id).expect("loop handle");
        let outcome = loops::drive_turn(
            self.deps.store.as_ref(),
            self.deps.ids.as_ref(),
            self.deps.clock.as_ref(),
            &ctx,
            &mut handle.child,
            &mut handle.phase,
            row.run_id,
            state,
            events,
            LOOP_CALL_TIMEOUT,
            self.deps.clock.now_unix_ms(),
        )
        .await;
        // A failed turn leaves the session's protocol state unknown — drop
        // the cached handle so the next tick spawns a fresh, handshaken
        // child instead of reusing a broken one.
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.stop_loop(row.run_id, "failed").await;
                return Err(error);
            }
        };
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
                self.stop_loop(row.run_id, "exited").await;
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
                let mut draft = contract::CreateApprovalRequest::decode(approval_draft.as_slice())
                    .map_err(|_| {
                        worker_error(
                            ErrorCode::InvalidArgument,
                            "approval draft is not a CreateApprovalRequest",
                        )
                    })?;
                if draft.expires_at_ms == 0 {
                    draft.expires_at_ms = self.deps.clock.now_unix_ms() + APPROVAL_TTL_MS;
                }
                self.submit(
                    approvals::CMD_CREATE_APPROVAL_REQUEST,
                    draft.encode_to_vec(),
                    format!("approval.{}.{}", row.run_id, row.step_sequence),
                )
                .await
            }
            // The effect row was prepared atomically at decision accept; the
            // `WaitingTool` arm claims, dispatches, and settles it.
            DecisionInstruction::InvokeEffect { .. } => Ok(()),
        }
    }

    /// Drives a `WaitingTool` run: in-flight effects are claimed,
    /// dispatched, and reconciled; once nothing is in-flight the run
    /// resumes (a settled effect or a fired wait timer both qualify).
    /// A run holding an `Unknown` effect stays parked — only
    /// `ResolveUnknownEffect` can exit that state.
    async fn step_waiting_tool(
        &mut self,
        row: &kernel_store::models::RunRow,
    ) -> errors::Result<()> {
        let effects = {
            let mut txn = self.read_txn().await?;
            txn.effects().list_by_run(row.run_id).await?
        };
        if effects
            .iter()
            .any(|effect| effect.state == EffectState::Unknown)
        {
            return Ok(());
        }
        let in_flight: Vec<EffectRow> = effects
            .into_iter()
            .filter(|effect| effects::is_in_flight(effect.state))
            .collect();
        if in_flight.is_empty() {
            if !{
                let mut txn = self.read_txn().await?;
                txn.effects().list_by_run(row.run_id).await?.is_empty()
            } || self.wait_expired(row).await?
            {
                return self.drive(row).await;
            }
            return Ok(());
        }
        for effect in in_flight {
            match effect.state {
                EffectState::Prepared | EffectState::Claimed => {
                    self.dispatch_effect(&effect).await?;
                }
                EffectState::Dispatched => {
                    self.maybe_reconcile_effect(&effect).await?;
                }
                EffectState::Acknowledged => self.commit_acknowledged(&effect).await?,
                _ => {}
            }
        }
        Ok(())
    }

    /// `Acknowledged -> Committed` — same-epoch commits verify the executor;
    /// rows acknowledged under a dead epoch commit via the durable
    /// acknowledgment (recovery matrix: no external call needed).
    async fn commit_acknowledged(&self, effect: &EffectRow) -> errors::Result<()> {
        let env = self.effect_env();
        let mut txn = self.write_txn().await?;
        let outcome = match (effect.executor_id.as_deref(), effect.executor_fencing_token) {
            (Some(executor_id), Some(token))
                if effect.daemon_fencing_epoch == Some(self.deps.epoch.epoch())
                    && effect
                        .lease_expires_ms
                        .is_some_and(|lease| lease > env.clock.now_unix_ms()) =>
            {
                effects::commit(
                    &mut *txn,
                    &env,
                    effect.effect_id,
                    &effects::ExecutorRef {
                        executor_id,
                        fencing_token: token,
                    },
                    effect.result_ref.clone(),
                )
                .await
            }
            _ => {
                effects::commit_durable(
                    &mut *txn,
                    &env,
                    effect.effect_id,
                    effect.result_ref.clone(),
                )
                .await
            }
        };
        match outcome {
            Ok(_) => txn.commit().await?,
            Err(error) => {
                txn.rollback().await.ok();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Claims then dispatches a `Prepared`/stale-`Claimed` effect to its
    /// bound adapter. `Dispatched` commits before the adapter call, so a
    /// lost response reconciles by operation id rather than re-dispatching.
    async fn dispatch_effect(&self, effect: &EffectRow) -> errors::Result<()> {
        let env = self.effect_env();
        let executor_id = self.executor_id();
        // 1. Claim (or reuse a claim this worker already holds).
        let fencing_token = {
            let mut txn = self.write_txn().await?;
            let outcome = effects::claim(&mut *txn, &env, effect.effect_id, &executor_id).await?;
            let token = match outcome {
                effects::ClaimOutcome::Claimed { fencing_token, .. } => Some(fencing_token),
                effects::ClaimOutcome::Busy(row)
                    if row.executor_id.as_deref() == Some(executor_id.as_str())
                        && row.daemon_fencing_epoch == Some(self.deps.epoch.epoch())
                        && row
                            .lease_expires_ms
                            .is_some_and(|lease| lease > env.clock.now_unix_ms()) =>
                {
                    row.executor_fencing_token
                }
                effects::ClaimOutcome::Busy(_) => None,
            };
            match token {
                Some(token) => {
                    // 2. Persist Dispatched before any external I/O.
                    effects::mark_dispatched(
                        &mut *txn,
                        &env,
                        effect.effect_id,
                        &effects::ExecutorRef {
                            executor_id: &executor_id,
                            fencing_token: token,
                        },
                        Some(effect.effect_id.to_string()),
                    )
                    .await?;
                    txn.commit().await?;
                    token
                }
                None => {
                    txn.rollback().await.ok();
                    return Ok(());
                }
            }
        };

        // 3. Spawn the bound adapter and call `execute`.
        let call = async {
            let (mut child, mut phase) = self.spawn_effect_adapter(effect).await?;
            let request = contract::PortCallRequest {
                call_id: format!("effect-{}", effect.effect_id),
                port_id: EFFECT_PORT_ID.to_owned(),
                operation: "execute".to_owned(),
                context: None,
                payload: contract::EffectExecutionRequest {
                    effect_id: effect.effect_id.to_string(),
                    operation_id: effect.effect_id.to_string(),
                    request_hash: effect.request_hash.clone(),
                    fencing_token,
                    payload: effect.request_payload.clone(),
                }
                .encode_to_vec(),
            };
            let response = session::dispatch_call(
                child.ipc(),
                &mut phase,
                request,
                Instant::now() + CALL_TIMEOUT,
            );
            let instance = child.instance;
            let reason = terminate(child, Duration::from_secs(2)).await;
            self.mark_instance_exit(instance, "exited", reason.to_string())
                .await;
            response
        }
        .await;

        // 4. Record the adapter's answer; a lost answer leaves the row
        //    Dispatched — the reconciler owns that ambiguity.
        let mut txn = self.write_txn().await?;
        let executor = effects::ExecutorRef {
            executor_id: &executor_id,
            fencing_token,
        };
        let outcome = match call {
            Ok(response) if response.error_code.is_empty() => {
                match contract::EffectExecutionResponse::decode(response.payload.as_slice()) {
                    Ok(decoded) if decoded.status == "succeeded" => {
                        effects::acknowledge(
                            &mut *txn,
                            &env,
                            effect.effect_id,
                            &executor,
                            Some(decoded.result_ref.clone()),
                        )
                        .await?;
                        effects::commit(
                            &mut *txn,
                            &env,
                            effect.effect_id,
                            &executor,
                            Some(decoded.result_ref),
                        )
                        .await
                    }
                    Ok(decoded) => {
                        effects::fail(
                            &mut *txn,
                            &env,
                            effect.effect_id,
                            &executor,
                            if decoded.error_code.is_empty() {
                                decoded.status
                            } else {
                                decoded.error_code
                            },
                        )
                        .await
                    }
                    Err(_) => {
                        return Err(worker_error(
                            ErrorCode::InvalidArgument,
                            "effect adapter response did not decode",
                        ));
                    }
                }
            }
            _ => {
                // Lost/failed call: leave Dispatched for reconciliation.
                txn.rollback().await.ok();
                return Ok(());
            }
        };
        match outcome {
            Ok(_) => txn.commit().await?,
            Err(error) => {
                txn.rollback().await.ok();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Reconciles a `Dispatched` effect once the dispatching executor is
    /// dead (epoch change) or its lease lapsed — queries the provider by
    /// the same operation id and applies the observed outcome.
    async fn maybe_reconcile_effect(&self, effect: &EffectRow) -> errors::Result<()> {
        let epoch_stale = effect.daemon_fencing_epoch != Some(self.deps.epoch.epoch());
        let lease_dead = effect
            .lease_expires_ms
            .is_some_and(|lease| lease <= self.deps.clock.now_unix_ms());
        let mine =
            effect.executor_id.as_deref() == Some(self.executor_id().as_str()) && !epoch_stale;
        if !mine && !epoch_stale && !lease_dead {
            return Ok(());
        }
        let env = self.effect_env();
        let call = async {
            let (mut child, mut phase) = self.spawn_effect_adapter(effect).await?;
            let operation_id = effect
                .provider_operation_ref
                .clone()
                .unwrap_or_else(|| effect.effect_id.to_string());
            let request = contract::PortCallRequest {
                call_id: format!("status-{}", effect.effect_id),
                port_id: EFFECT_PORT_ID.to_owned(),
                operation: "status".to_owned(),
                context: None,
                payload: contract::EffectStatusRequest {
                    effect_id: effect.effect_id.to_string(),
                    operation_id: operation_id.clone(),
                    provider_operation_ref: operation_id,
                }
                .encode_to_vec(),
            };
            let response = session::dispatch_call(
                child.ipc(),
                &mut phase,
                request,
                Instant::now() + CALL_TIMEOUT,
            );
            let instance = child.instance;
            let reason = terminate(child, Duration::from_secs(2)).await;
            self.mark_instance_exit(instance, "exited", reason.to_string())
                .await;
            response
        }
        .await;
        // An unreachable provider is transient — leave Dispatched for the
        // next tick rather than parking the run on a flaky observation.
        let Ok(response) = call else {
            tracing::warn!(effect = %effect.effect_id, "effect status call failed");
            return Ok(());
        };
        if !response.error_code.is_empty() {
            tracing::warn!(effect = %effect.effect_id, error = %response.error_code, "effect status returned error");
            return Ok(());
        }
        let observed = match contract::EffectStatusResponse::decode(response.payload.as_slice()) {
            Ok(decoded) => match decoded.status.as_str() {
                "succeeded" => effects::ObservedOutcome::Succeeded {
                    result_ref: decoded.result_ref,
                },
                "failed" => effects::ObservedOutcome::Failed {
                    error_code: decoded.error_code,
                },
                "not_found" => effects::ObservedOutcome::NotFound,
                _ => effects::ObservedOutcome::Unknown,
            },
            Err(_) => effects::ObservedOutcome::Unknown,
        };
        let mut txn = self.write_txn().await?;
        let plan = effects::reconcile_plan(effect, observed);
        let outcome = effects::apply_observed(&mut *txn, &env, effect.effect_id, plan).await;
        match outcome {
            Ok(_) => txn.commit().await?,
            Err(error) => {
                txn.rollback().await.ok();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Spawns the fixture effect adapter a prepared effect binds to,
    /// verifies the bundle digest, and completes the handshake.
    async fn spawn_effect_adapter(
        &self,
        effect: &EffectRow,
    ) -> errors::Result<(Child, SessionPhase)> {
        let adapter_id = effect.adapter_id.to_string();
        let version = effect.adapter_version.clone();
        let bundle = self
            .deps
            .bundles
            .iter()
            .find(|bundle| {
                bundle.adapter_id == adapter_id
                    && bundle.version == version
                    && bundle.bundle_digest == effect.adapter_digest
            })
            .cloned()
            .ok_or_else(|| {
                worker_error(
                    ErrorCode::FailedPrecondition,
                    "no local bundle for the frozen effect adapter",
                )
            })?;
        let adapter_uuid = AdapterId::from_str(&adapter_id).map_err(|_| {
            worker_error(
                ErrorCode::Internal,
                "effect adapter id is not an adapter id",
            )
        })?;
        let verified = {
            let mut txn = self.write_txn().await?;
            let verified = verify_for_spawn(
                &mut *txn,
                adapter_uuid,
                &version,
                &effect.adapter_digest,
                &bundle.dir,
            )
            .await;
            txn.rollback().await.ok();
            verified?
        };
        let mut env = vec![
            ("FIXTURE_ADAPTER_ID".to_owned(), adapter_id.clone()),
            ("FIXTURE_ADAPTER_VERSION".to_owned(), version.clone()),
            (
                "FIXTURE_STORE".to_owned(),
                self.deps
                    .runtime_dir
                    .join(format!("fixture-store-{adapter_id}.json"))
                    .to_string_lossy()
                    .to_string(),
            ),
        ];
        for (key, value) in &self.deps.effect_env {
            env.push((key.clone(), value.clone()));
        }
        if let Ok(level) = std::env::var("RUST_LOG") {
            env.push(("RUST_LOG".to_owned(), level));
        }
        // Effect adapters that call out to providers (e.g. OpenRouter)
        // read credentials/config from these daemon env vars; propagate
        // only this allowlist.
        for key in [
            "OPENROUTER_API_KEY",
            "OPENROUTER_MODEL",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_SITE",
            "OPENROUTER_APP_NAME",
            "OPENROUTER_TIMEOUT_MS",
            "OPENROUTER_ALLOW_ANY_BASE_URL",
            // MCP tool servers configured for `mcp.*` effect payloads.
            "MCP_SERVERS",
            "MCP_TIMEOUT_MS",
            "MCP_ALLOW_ANY_URL",
        ] {
            if let Ok(value) = std::env::var(key)
                && !value.is_empty()
            {
                env.push((key.to_owned(), value));
            }
        }
        // `PROVIDER_KEY_*` holds per-provider credentials referenced by
        // `api_key_env` in model.chat payloads — forwarded by name, so
        // arbitrary daemon secrets still never reach adapters.
        for (key, value) in std::env::vars() {
            if key.starts_with("PROVIDER_KEY_") && !value.is_empty() {
                env.push((key, value));
            }
        }
        let adapter_instance = AdapterInstanceId::new(self.deps.ids.as_ref());
        let (executable, argv, ipc) = spawn_target(&bundle, verified.entrypoint)?;
        let mut child = spawn::spawn(&SpawnSpec {
            adapter_id: adapter_uuid,
            adapter_version: version.clone(),
            expected_bundle_digest: effect.adapter_digest.clone(),
            adapter_instance_id: adapter_instance,
            daemon_instance_id: self.deps.daemon_instance,
            daemon_fencing_epoch: self.deps.epoch.epoch(),
            protocol_version: 1,
            executable,
            argv,
            env,
            cwd: None,
            isolation: bundle.isolation,
            stdout_ipc: ipc,
        })?;
        child.drain_output();
        if let Err(error) = self
            .record_instance_spawn(
                adapter_uuid,
                &version,
                &effect.adapter_digest,
                adapter_instance,
                child.pid,
                &child.start_identity,
            )
            .await
        {
            let _ = terminate(child, Duration::from_secs(2)).await;
            return Err(error);
        }
        let nonce = self.deps.ids.new_uuid_v7().to_string();
        let identity = ExpectedIdentity {
            daemon_instance_id: self.deps.daemon_instance,
            daemon_fencing_epoch: self.deps.epoch.epoch(),
            adapter_instance_id: adapter_instance.to_string(),
            adapter_id: adapter_id.clone(),
            adapter_version: version,
            expected_bundle_digest: effect.adapter_digest.clone(),
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
                worker_error(ErrorCode::Unavailable, "effect ipc timeout setup failed")
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
                self.mark_instance_exit(adapter_instance, "failed", "handshake failed".to_owned())
                    .await;
                let _ = terminate(child, Duration::from_secs(2)).await;
                return Err(worker_error(
                    ErrorCode::Unavailable,
                    "effect adapter did not complete the handshake",
                ));
            }
        };
        if let Err(error) = self.mark_instance_ready(adapter_instance).await {
            let _ = terminate(child, Duration::from_secs(2)).await;
            return Err(error);
        }
        Ok((child, phase))
    }

    /// Read the requested profile's `agent_loop` binding under the
    /// active generation and turn it into an adapter pin for `resolve`.
    /// `Ok(None)` for builtin names (e.g. `fixture-loop`) which carry no
    /// adapter id; errors surface before any execution side effect.
    async fn profile_loop_pin(
        &self,
        txn: &mut dyn KernelTxn,
        profile_name: &str,
    ) -> errors::Result<Option<AdapterId>> {
        let Some(active) = txn.config().get_active().await? else {
            return Ok(None);
        };
        let Some(generation) = txn.config().get_generation(active.generation_id).await? else {
            return Ok(None);
        };
        let doc = config_engine::model::parse_document(
            std::str::from_utf8(&generation.document)
                .map_err(|_| worker_error(ErrorCode::Internal, "generation doc not utf-8"))?,
        )?;
        let profile = config_engine::profile::resolve_profile(&doc, profile_name)?;
        let Some(binding) = profile.bindings.get(LOOP_PORT_ID) else {
            return Ok(None);
        };
        let name = binding.split('@').next().unwrap_or(binding);
        if config_engine::generations::BUILTIN_ADAPTERS.contains(&name) {
            return Ok(None);
        }
        name.parse::<AdapterId>().map(Some).map_err(|_| {
            worker_error(
                ErrorCode::InvalidArgument,
                "profile agent_loop binding is not an adapter id",
            )
        })
    }

    /// The executor identity this worker claims effects under.
    fn executor_id(&self) -> String {
        format!("agentd.run-worker.{}", self.deps.epoch.epoch())
    }

    /// Ambient effect-operation dependencies.
    fn effect_env(&self) -> effects::EffectEnv<'_> {
        effects::EffectEnv {
            ids: self.deps.ids.as_ref(),
            clock: self.deps.clock.as_ref(),
            correlation_id: None,
            causation_id: None,
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

    /// `limits.context` for the run's frozen environment, cached by env
    /// id; absent env/generation yields the built-in defaults.
    async fn context_limits_for(
        &mut self,
        row: &kernel_store::models::RunRow,
    ) -> errors::Result<config_engine::schema::ContextLimits> {
        let Some(env_id) = row.resolved_environment_id else {
            return Ok(config_engine::schema::ContextLimits::default());
        };
        if let Some(limits) = self.context_limits.get(&env_id) {
            return Ok(limits.clone());
        }
        // The environment is frozen state: a missing env/generation row
        // or an unparseable document is durable corruption — stop the run
        // rather than silently widening the context budget to defaults.
        let limits = {
            let mut txn = self.read_txn().await?;
            let env = txn
                .environments()
                .get_environment(env_id)
                .await?
                .ok_or_else(|| {
                    worker_error(ErrorCode::Internal, "frozen environment row is missing")
                })?;
            let generation = txn
                .config()
                .get_generation(env.config_generation_id)
                .await?
                .ok_or_else(|| {
                    worker_error(
                        ErrorCode::Internal,
                        "frozen config generation row is missing",
                    )
                })?;
            std::str::from_utf8(&generation.document)
                .ok()
                .and_then(|s| config_engine::model::parse_document(s).ok())
                .map(|d| d.limits.context)
                .ok_or_else(|| {
                    worker_error(ErrorCode::Internal, "frozen config document is unreadable")
                })?
        };
        self.context_limits.insert(env_id, limits.clone());
        Ok(limits)
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
            .find(|bundle| {
                bundle.adapter_id == adapter_id
                    && bundle.version == version
                    && bundle.bundle_digest == digest
            })
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
            ("AGENTOS_ADAPTER_ID".to_owned(), adapter_id.clone()),
            ("AGENTOS_ADAPTER_VERSION".to_owned(), version.clone()),
        ];
        for (key, value) in &self.deps.loop_env {
            env.push((key.clone(), value.clone()));
        }
        if let Ok(level) = std::env::var("RUST_LOG") {
            env.push(("RUST_LOG".to_owned(), level));
        }
        // LLM loop adapters read their credentials/config from these
        // daemon env vars; propagate only this allowlist.
        for key in [
            "OPENROUTER_API_KEY",
            "OPENROUTER_MODEL",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_SITE",
            "OPENROUTER_APP_NAME",
            "OPENROUTER_TIMEOUT_MS",
            "OPENROUTER_ALLOW_ANY_BASE_URL",
            // ACP loop adapter: which agent CLI to spawn and how.
            "ACP_COMMAND",
            "ACP_ARGS",
            "ACP_CWD",
            "ACP_TIMEOUT_MS",
            "ACP_ALLOW_TOOLS",
            // Test/debug knob: the fixture loop records each LoopInput.
            "FIXTURE_LOOP_RECORD_DIR",
        ] {
            if let Ok(value) = std::env::var(key)
                && !value.is_empty()
            {
                env.push((key.to_owned(), value));
            }
        }
        let (executable, argv, ipc) = spawn_target(&bundle, verified.entrypoint)?;
        let mut child = spawn::spawn(&SpawnSpec {
            adapter_id: adapter_uuid,
            adapter_version: version.clone(),
            expected_bundle_digest: digest.clone(),
            adapter_instance_id: adapter_instance,
            daemon_instance_id: self.deps.daemon_instance,
            daemon_fencing_epoch: self.deps.epoch.epoch(),
            protocol_version: 1,
            executable,
            argv,
            env,
            cwd: None,
            isolation: bundle.isolation,
            stdout_ipc: ipc,
        })?;
        child.drain_output();
        if let Err(error) = self
            .record_instance_spawn(
                adapter_uuid,
                &version,
                &digest,
                adapter_instance,
                child.pid,
                &child.start_identity,
            )
            .await
        {
            let _ = terminate(child, Duration::from_secs(2)).await;
            return Err(error);
        }

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
                self.mark_instance_exit(adapter_instance, "failed", "handshake failed".to_owned())
                    .await;
                let _ = terminate(child, Duration::from_secs(2)).await;
                return Err(worker_error(
                    ErrorCode::Unavailable,
                    "loop adapter did not complete the handshake",
                ));
            }
        };
        if let Err(error) = self.mark_instance_ready(adapter_instance).await {
            let _ = terminate(child, Duration::from_secs(2)).await;
            return Err(error);
        }
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

/// Handler for `agentos.internal.RunCancelComplete` — the worker that owns
/// a `Cancelling` run's in-flight adapter work has stopped it, so the run
/// terminalizes to `Cancelled` in one CAS + event commit.
pub struct RunCancelCompleteHandler {
    ids: Arc<dyn IdProvider>,
}

impl RunCancelCompleteHandler {
    /// Builds the handler with the daemon's id provider for event minting.
    pub fn new(ids: Arc<dyn IdProvider>) -> Self {
        Self { ids }
    }
}

#[async_trait::async_trait]
impl CommandHandler for RunCancelCompleteHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let text = String::from_utf8(payload).map_err(|_| {
            worker_error(
                ErrorCode::InvalidArgument,
                "cancel-complete payload is not utf-8",
            )
        })?;
        let run_id = RunId::from_str(&text).map_err(|_| {
            worker_error(
                ErrorCode::InvalidArgument,
                "cancel-complete payload is not a run id",
            )
        })?;
        let _ = ctx;
        let run = txn
            .runs()
            .get(run_id)
            .await?
            .ok_or_else(|| worker_error(ErrorCode::NotFound, "run not found"))?;
        if run.state.is_terminal() {
            // Idempotent replay: a previously finalized cancel is a no-op.
            return Ok(CommandOutcome {
                code: OutcomeCode::Ok,
                payload: Vec::new(),
            });
        }
        if run.state != RunState::Cancelling {
            return Err(worker_error(
                ErrorCode::FailedPrecondition,
                "run is not cancelling",
            ));
        }
        let moved = txn
            .runs()
            .cas_update(
                run_id,
                RunCas {
                    run_revision: run.run_revision,
                    state: Some(RunState::Cancelling),
                    cancellation_epoch: None,
                },
                RunPatch {
                    state: Some(RunState::Cancelled),
                    terminal_reason: Some(runtime::state::REASON_CANCELLED.to_owned()),
                    bump_revision: true,
                    ..RunPatch::default()
                },
            )
            .await?;
        if !moved {
            return Err(worker_error(
                ErrorCode::Conflict,
                "run moved during cancel finalization",
            ));
        }
        for event_type in ["RunCancelled", "RunStateChanged"] {
            runtime::stage_catalogued(
                txn,
                domain::ids::EventId::new(self.ids.as_ref()),
                event_type,
                events::StreamKey::run(run_id),
                run_id.to_string().into_bytes(),
                ctx.correlation_id.clone(),
                None,
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
