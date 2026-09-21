# Task LOOP-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** LOOP-001 — Deterministic agent-loop fixture (private protocol)
- **Status:** DONE
- **Commits:** `c273829` — `feat(fixtures+conformance): effect/loop fixture bundles, behavioral conformance packs, activation gate [ADP-005][ADP-006][LOOP-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `fixtures/agent-loop` (`fixture-agent-loop`): fd-0 socketpair handshake
  identical to the effect fixture; each `PortCallRequest` carries a
  `LoopStepInput` and returns the next `AgentLoopDecision` from
  `FIXTURE_LOOP_SCRIPT` (JSON array indexed by `step_sequence` —
  fully deterministic, no model calls).
- Every decision echoes the run identity it was given — `run_id`,
  `run_revision`, `loop_epoch`, `step_sequence`, `cursor`, `turn` — plus
  a deterministic `decision_id` (`dec-<step>`), so the supervisor can
  assert fenced echo correctness.
- Decision vocabulary: `complete`, `fail`, `wait`, `spawn_agent`,
  `invoke_effect`, `request_approval` (payload fields base64-decoded
  inline). Script overflow -> `fail(script_exhausted)`.
- Flags: `FIXTURE_LOOP_DELAY_MS`, `FIXTURE_LOOP_STALE` (echoes
  `loop_epoch - 1`), `FIXTURE_LOOP_CRASH`. Immutable bundle:
  `adapter.manifest.json` (`agent_loop@1`) + lock.
- AC met: the fixture has no process-spawn capability and requests no
  privileged effects — `spawn_agent` is a *decision payload* the loop
  supervisor interprets, never a direct spawn.

## Evidence

`cargo test -p fixture-agent-loop` (4 tests): scripted complete decision
echoes all fenced fields; spawn-agent + effect + wait + approval
decisions round-trip; delayed and stale-epoch flags behave; bundle
digest validates through the registry path.
