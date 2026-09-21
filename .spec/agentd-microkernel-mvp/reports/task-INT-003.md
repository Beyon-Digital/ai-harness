# INT-003 — Verify effect crash/restart and reconciliation end-to-end

## What was built

`crates/agentd/tests/e2e_effect_recovery.rs` — three process-level scenarios
over a real booted daemon + fixture effect adapter (`fixtures/effect-adapter`,
`effect.execute@1`):

- `e2e_effect_executes_normally` — `invoke_effect` decision → WaitingTool →
  claim → fenced dispatch → ack → commit → run resumes → Completed; the
  fixture store records exactly one mutation.
- `e2e_crash_before_ack_reconciles_exactly_once` —
  `FIXTURE_CRASH_BEFORE_RESPONSE=1` makes the adapter apply the side effect
  then `exit(2)` before writing its response. The daemon `abort()`s while the
  row sits `Dispatched`; a second daemon boots the same runtime dir under a
  higher fencing epoch, startup recovery classifies the run
  `NeedsReconciliation`, and the worker's status call observes the recorded
  `succeeded` outcome for the same `operation_id` → `Committed` → run
  completes. Fixture store still shows exactly one mutation — no duplicate
  dispatch.
- `e2e_unreconcilable_effect_blocks_until_resolved` — same crash shape on
  `fixture.unreconcilable_counter` (kernel-declared `Impossible` +
  `NotIdempotent`): startup recovery flips the row `Unknown` and the run to
  `BLOCKED_UNKNOWN_EFFECT`; the run stays parked until `agentctl
  resolve-effect --action mark_succeeded` settles it, after which the run
  completes.

## Supporting changes (implementation the task exposed)

- `crates/agentd/src/workers/runs.rs` — `WaitingTool` runs now drive the
  effect lifecycle: `step_waiting_tool` (dispatch Prepared/Claimed,
  reconcile Dispatched, commit Acknowledged, park on Unknown, resume once
  the effect row settles), `dispatch_effect`, `maybe_reconcile_effect`,
  `commit_acknowledged`, `spawn_effect_adapter` (verified bundle + identity
  handshake + `FIXTURE_STORE` under the runtime dir so provider state
  survives daemon restart).
- `restake_claim` — a `WaitingTool`/`WaitingChild` run whose claim epoch is
  dead is re-staked under the live epoch **without a state change**;
  previously only `Running` runs were reclaimed, so waiting runs could
  never resume after restart (real bug found by this task).
- `crates/effects` — `commit_durable` (Acknowledged→Committed without
  executor verification, for rows acknowledged under a dead epoch) and
  `policy::kernel_declared` (kernel-known semantics for the two fixture
  ops so the conservative resolver yields contracts without a conformance
  run).
- `crates/runtime/src/decision.rs` — `InvokeEffect` decisions atomically
  `prepare_effect` + move the run to `WaitingTool`, resolving the
  `effect.execute` binding from the run's frozen environment.
- `crates/config-engine` + `config/default.yaml` — new fixed profile slot
  `effect_execute`; `local-trusted` binds the fixture effect adapter.
- `crates/agentd/src/bootstrap.rs` — `Daemon::abort()` crash simulation
  (worker/server abort, waits for the control socket to die so reboot can't
  race the listener); seeds the daemon-owner operator delegation chain
  (`effect.resolve_unknown` grant) so `ResolveUnknownEffect` is authorized
  for the local peer principal; `DaemonConfig.effect_env` injects fixture
  fault flags into spawned effect adapters.
- `crates/control-api` — `submit_command` attaches the operator chain as
  `delegation_chain_id` (the local owner *is* the operator in MVP auth).

## Verification

- `cargo test -p agentd --test e2e_effect_recovery` — 3/3 pass (4× runs)
- `cargo test -p agentd --test e2e_children_workspace` — 3/3 pass (2×)
- `cargo test --workspace` — all green (129 test binaries)
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `validate_buildpack.py`, `check-contract-mirror.sh`, `validate_repo.py` — OK

## Acceptance criteria

- [x] No test permits automatic duplicate mutation after ambiguous dispatch
      — the crashed `Dispatched` row is only ever settled by a `status`
      observation keyed on the same `operation_id`, or operator-resolved
      from `Unknown`; the fixture store proves exactly one mutation.

## Follow-ups (non-blocking)

- `restake_claim` doesn't bump `loop_epoch` — a waiting run's pending
  decision is already fenced by step/decision ids; revisit if a real
  adapter can emit a decision while the run waits.
