> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Common Error Model

Errors carry stable code, category, message, retry policy (`never`, `safe_same_operation`, `conditional`, `after_reconciliation`), details, correlation ID, and optional effect/run references.

Categories: validation/config, authz/approval, stale revision/fence, unsupported capability, conflict, transient backend, rate/resource, timeout/cancelled, unknown external outcome, adapter protocol violation, invariant/internal failure.
