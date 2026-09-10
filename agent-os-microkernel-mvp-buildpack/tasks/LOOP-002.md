# LOOP-002 — Implement loop turn supervisor and fenced decision acceptance

**Phase:** `loop`  
**Depends on:** [LOOP-001](LOOP-001.md), [RUN-005](RUN-005.md), [EFF-002](EFF-002.md), [RUN-004](RUN-004.md)

## Goal

Issue turns to resolved loop process and atomically accept only current decisions.

## Required outputs / expected code locations

- `crates/runtime/src/loop_turn.rs`
- `crates/runtime/src/decision.rs`
- `crates/agentd/src/workers/loops.rs`

## Normative inputs

- Use the component specs linked from the goal/steps and `DECISIONS.md`.

## Implementation steps

1. Create durable/derivable issued turn containing turn ID and current fence tuple.
2. Send LoopInput to exact frozen loop bundle/process.
3. Validate response IDs/fence tuple.
4. On accepted decision, one transaction advances run/cursor/revision and prepares effect or creates child/wait state.
5. Duplicate decision ID for same accepted turn returns prior outcome; stale mismatches reject with no side effect.
6. Restarting/rebinding loop increments loop_epoch.

## Required tests

- `stale revision/epoch/step/cursor each rejected`
- `duplicate decision does not duplicate effect/child`
- `loop crash increments/recovery behavior`

## Acceptance criteria

- [ ] Delayed response from old epoch can never affect current run.

## Forbidden shortcuts

- No task-specific additions beyond `AGENTS.md` prohibited shortcuts.

## Completion evidence

Record in `TASK_STATUS.yaml`:

- exact test commands executed;
- pass/fail result;
- notable concurrency/fault-injection seed if relevant;
- git commit SHA if available;
- any non-blocking follow-up explicitly outside this task.
