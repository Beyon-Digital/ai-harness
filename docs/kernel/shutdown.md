> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Shutdown Coordinator

Correct graceful shutdown order:

1. Stop accepting new state-changing external commands.
2. Mark daemon draining.
3. Stop scheduling new runs/effects.
4. Stop issuing new loop turns.
5. Drain/cancel in-flight operations according to policy.
6. Stop/terminate supervised children and sandboxes where appropriate.
7. Reconcile late completion responses/effects.
8. Commit final state/outbox records.
9. Flush outbox publisher to configured checkpoint target.
10. Persist checkpoints.
11. Release daemon lease/fence authority.
12. Close stores.

State is not “flushed first and processes killed afterward.”
