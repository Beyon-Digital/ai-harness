> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Recovery Coordinator

Startup recovery acquires a fresh daemon fencing epoch, loads active config, reconstructs non-terminal runs/graph, reconciles stale process/sandbox/effect/resource claims, and assigns `RecoveryDisposition`.

Unknown external effects are inspected/reconciled if possible. Non-reconcilable unknown effects block automatic retry and follow configured human/fail/duplicate-risk policy.

Recovery reconstructs state; it does not re-run nondeterministic history to “prove” the same result.
