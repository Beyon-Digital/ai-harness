# INT-001 — Compose `agentd` (single-binary kernel)

## What was built

- `crates/agentd/src/lib.rs` exposes the daemon internals (`api`, `bootstrap`, `lock`, `recovery`, `workers`) so the binary and integration tests share one composition.
- `crates/agentd/src/bootstrap.rs` — `boot(DaemonConfig)` composes the real stack in order: runtime-dir creation → `DaemonLock` (singleton) → `SqliteKernelStore::open` → `acquire_daemon_fence` (epoch) → `run_startup_recovery` → command registry (runtime + approvals + config-engine + internal `RunWaitExpired`) → `CommandCoordinator` (fenced write txns) → `SqliteEventJournal` + `LiveBus` + `EventDispatcher` → adapter bundle registration (idempotent across restarts) → config bootstrap (propose → mark-tested → activate when no active generation) → `OutboxWorker` + `SchedulerWorker` + `RunWorker` on a shutdown watch channel → `UidPrincipalMap` + `ControlApiService` + `EventApiService` over `ControlSocket`. `Daemon` owns the lock for its lifetime and provides `initiate_shutdown()` (drain-first) + `wait()`.
- `crates/agentd/src/main.rs` — CLI: `--runtime-dir`, `--config`, repeatable `--adapter-bundle`, `--json`; SIGINT/SIGTERM → `initiate_shutdown` → `wait`.
- `crates/agentd/src/workers/runs.rs` — `RunWorker` drives the run lifecycle end-to-end: `Created` → plan env + submit `BindRun` → `Ready` → `ClaimReadyRun` → `Running` → spawn/verify fixture loop bundle (digest + handshake) → `drive_turn` → apply `DecisionInstruction` (Complete/Fail terminate the child; Wait schedules a `RunWaitExpired` timer; SpawnAgent submits `CreateTaskRun`; RequestApproval submits `CreateApprovalRequest`) → reclaim stale claims under a bumped `loop_epoch`.
- `crates/runtime/src/bind.rs` — new internal command `agentos.spec.v1.BindRun`: decodes `contract::ResolvedRunEnvironment`, freezes the environment + bindings, CASes `Created → Ready`, emits `RunReady`/`RunBound`/`RunStateChanged`.
- Binding inputs (`agent_spec_id/version/digest`, `requested_profile`, `workspace_uri`) are captured immutably on `runs` at `CreateTaskRun` so the binder resolves without the caller re-supplying them (schema, models, sqlite repo, mock store).
- `accept_decision` now emits the catalogued lifecycle events (`LoopDecisionAccepted` + `RunCompleted`/`RunFailed`/`RunWaiting{Tool,Child,Human}` + `RunStateChanged`) per the command/event catalogs; replayed `decision_id` returns the recorded outcome without re-staging.
- Fixture loop: `decision_id` echoes the issued `turn_id` — deterministic per turn, UUID-shaped, so a redispatched turn replays instead of double-committing.

## Tests (required by the task)

`crates/agentd/tests/e2e_basic.rs` — no test reaches into the store; everything goes through the real daemon + `agentctl` over the UDS API:

- `e2e_complete_run_via_daemon_api` — boots a real daemon with `config/default.yaml` + a content-addressed fixture-loop bundle dir; `create-session` → `put-agent-spec` → `create-run --profile local-trusted`; the worker binds, claims, spawns, and drives the run to `Completed`; asserts `resolved_environment_id` set and `run/<id>` stream shows `RunReady … RunCompleted`.
- `e2e_restart_after_completed_run_retains_audit_data` — completes a run, clean-shuts down, reboots on the same runtime dir (new fencing epoch), asserts `get-run` still returns `Completed` and the journal stream is intact.
- `e2e_idle_shutdown` — boot → shutdown while idle → `wait` succeeds; singleton released so a second daemon can boot the same dir.

## Notes / deviations

- `InvokeEffect` in `follow_up` returns "not wired yet" — deferred to INT-003 per its own scope.
- Adapter IPC calls block a worker thread briefly per turn (`std::os::unix::net::UnixStream` with deadlines) — acceptable at test scale; async transport is post-MVP.
