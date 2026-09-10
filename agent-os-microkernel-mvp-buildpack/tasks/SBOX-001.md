# SBOX-001 — Implement Sandbox Manager and T0 trusted local-process adapter

**Phase:** `sandbox`  
**Depends on:** [ADP-004](ADP-004.md), [WRK-003](WRK-003.md), [SEC-004](SEC-004.md)

## Goal

Provide trusted local execution plus strict tier resolution semantics.

## Required outputs / expected code locations

- `crates/sandbox/src/manager.rs`
- `crates/sandbox/src/local_process.rs`

## Normative inputs

- Use the component specs linked from the goal/steps and `DECISIONS.md`.

## Implementation steps

1. Resolve sandbox adapter by required trust tier.
2. Implement T0 process exec with cwd, env allowlist, deadlines, cancellation, stdout/stderr capture.
3. Integrate workspace lease/resource URI checks.
4. Pass only broker-approved secret material.
5. Reject T1/T2/T3 when no adapter claims/passes those capabilities; especially no T2->T0 fallback.

## Required tests

- `T0 command execution`
- `deadline/cancel`
- `env allowlist`
- `T2 fail-closed`
- `revoked workspace lease prevents coordinator-mediated exec/write path`

## Acceptance criteria

- [ ] Documentation/API reports T0 is not a security boundary.

## Forbidden shortcuts

- No task-specific additions beyond `AGENTS.md` prohibited shortcuts.

## Completion evidence

Record in `TASK_STATUS.yaml`:

- exact test commands executed;
- pass/fail result;
- notable concurrency/fault-injection seed if relevant;
- git commit SHA if available;
- any non-blocking follow-up explicitly outside this task.
