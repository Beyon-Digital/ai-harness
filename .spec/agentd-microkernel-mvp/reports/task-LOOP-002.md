# Task LOOP-002 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** LOOP-002 — Durable loop turns: fenced issue, decision accept, idempotent replay, epoch restart
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `runtime/src/loop_turn.rs`: `issue_turn` CAS-advances the run
  (Running, step+1, revision+1) and durably inserts the `issued` turn
  carrying the post-bump fence tuple — a crash between issue and accept
  leaves a reconcile-able record. `bump_loop_epoch` increments
  `loop_epoch` and stales outstanding issued turns (restart/rebind).
- `runtime/src/decision.rs`: `accept_decision` replays a known
  `decision_id` with zero side effects; validates the fence tuple
  (revision/epoch/step/cursor/turn) — mismatches mark the turn `stale`
  and reject with `Conflict`; valid decisions insert the decision row,
  CAS the turn `issued→accepted`, and move the run to the implied state
  while returning the follow-up instruction (effect claim bytes, child
  request, wait timer, approval draft) — no duplicate effect/child on
  replay.
- `agentd/src/workers/loops.rs`: `drive_turn` — issue txn, `LoopInput`
  over the supervised socketpair (`agent_loop.next`), decode, accept
  txn. Composition wiring lands with INT-001.
- `adapter-protocol`: `SessionPhase` re-exported for workers.

## Evidence

`cargo test -p runtime --test loop_decision` (5 tests): issue→accept
advances revision/step and transitions run state; duplicate decision_id
replays the recorded row with the run unmoved; stale
revision/epoch/step/cursor all rejected and the turn marked stale;
epoch bump stales issued turns and the next turn carries the new epoch;
an issued turn left by a crash is visible + staleable by recovery.
