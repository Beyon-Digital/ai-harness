# LOOP-001 — Build fixture external AgentLoop process

**Phase:** `loop`  
**Depends on:** [ADP-004](ADP-004.md), [RUN-001](RUN-001.md)

## Goal

Provide a deterministic external loop fixture that can emit Complete, SpawnAgent, Wait, and InvokeEffect decisions.

## Required outputs / expected code locations

- `fixtures/agent-loop/**`

## Normative inputs

- Use the component specs linked from the goal/steps and `DECISIONS.md`.

## Implementation steps

1. Use private adapter protocol/AgentLoop protobuf.
2. Fixture behavior driven by deterministic script in run input/test config.
3. Echo exact run revision/epoch/step/cursor/turn and generate decision ID.
4. Support intentional delayed/stale responses and crash/restart flags.
5. Package as immutable fixture bundle.

## Required tests

- `normal complete decision`
- `spawn child decision`
- `effect decision`
- `delayed decision fixture`
- `bundle digest validation`

## Acceptance criteria

- [ ] Fixture never directly spawns processes or performs privileged side effects.

## Forbidden shortcuts

- No task-specific additions beyond `AGENTS.md` prohibited shortcuts.

## Completion evidence

Record in `TASK_STATUS.yaml`:

- exact test commands executed;
- pass/fail result;
- notable concurrency/fault-injection seed if relevant;
- git commit SHA if available;
- any non-blocking follow-up explicitly outside this task.
