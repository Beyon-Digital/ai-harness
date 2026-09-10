> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Scheduler

Timers/jobs use durable versioned records and CAS-based linearization.

```text
Scheduled → Claimed → Fired
          └────────→ Cancelled
```

Cancel and claim both conditionally transition from the same expected version, so only one wins. Scheduler workers are fenced and create normal kernel commands/effects rather than bypassing the runtime.
