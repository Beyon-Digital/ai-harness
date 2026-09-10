> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Runtime Manager

## Run state

```text
Created → Ready → Running
                 ├→ WaitingTool → Running
                 ├→ WaitingChild → Running
                 ├→ WaitingHuman → Running
                 ├→ Suspended → Running
                 ├→ Cancelling → Cancelled
                 └→ Completed | Failed
```

`RecoveryDisposition` is separate from execution state: `Normal`, `NeedsReconciliation`, `Recovering`, `BlockedUnknownEffect`, `BlockedMissingResource`, `RequiresHumanDecision`.

## Fencing fields

Every run tracks `run_revision`, `loop_epoch`, `step_sequence`, and durable input cursor. A loop decision must match all expected values before acceptance.

## Operations

Create/start/pause/resume/cancel/fail/complete/retry task; create child; transition waiting states; bind/resume loop; record terminal reason.
