> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# AgentLoop

The loop is replaceable per run and supervised by the kernel.

## Input fencing

`run_id`, `run_revision`, `loop_epoch`, `step_sequence`, `input_event_cursor`, `turn_id`, and loop-private checkpoint reference.

## Output

`decision_id` plus the exact expected revision/epoch/step/cursor and one kernel-recognized decision/intent.

## Acceptance

Kernel CAS-validates all fencing fields and atomically advances the cursor/revision while creating the requested effect/child/wait state/outbox events.

## Typical decisions

`CallModel`, `InvokeTool`, `SpawnAgent`, `ReadMemory`, `WriteMemory`, `RequestContext`, `WaitForRuns`, `SendMessage`, `Complete`, `Fail`.

A loop never directly authorizes process spawn, secret access, or authoritative graph mutation.
