> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Effect Semantics Reference

Effect dimensions are independent: mutation class, idempotency, reconciliation, cancellation, and compensation. Never collapse these into a single vague “retryable” boolean.

`Unknown` is an uncertainty state requiring reconciliation or explicit policy; it is not equivalent to `Failed`.
