> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Failure and Recovery Model

The MVP assumes process crashes and lost acknowledgements are normal failure modes.

## Failure classes

- **Before commit:** no authoritative mutation occurred; retry command using same idempotency key.
- **After KernelStore commit, before response:** replay returns stored idempotency outcome.
- **After outbox commit, before journal append:** dispatcher retries projection.
- **After journal append, before outbox mark:** exact reappend is idempotent.
- **Effect dispatched, response lost:** reconcile by operation ID if supported; otherwise mark `Unknown`.
- **External process crash:** supervisor records health/exit; authoritative run/effect handling follows coordinator policy.
- **Daemon crash:** new instance acquires higher daemon fencing epoch, reconstructs non-terminal state, reconciles claimed effects/timers/resources.

## Recovery does not imply re-execution

Recovery reconstructs persisted execution truth and safely continues where semantics allow. It does not promise identical LLM/web/tool results.
