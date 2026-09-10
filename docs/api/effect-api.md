> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Effect API

Kernel/internal extension API exposes status-oriented effect controls rather than arbitrary retries.

Representative operations:
- query `EffectRecord`.
- request reconciliation.
- request cancellation where supported.
- resolve a blocked `Unknown` according to authorized policy (fail, accept duplicate-risk redispatch, mark externally verified outcome).

Ordinary agent loops do not directly mutate effect state; they receive resulting runtime events/decisions.
