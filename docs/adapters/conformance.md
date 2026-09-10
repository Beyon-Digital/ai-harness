> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Adapter Conformance Testing

Conformance gates registration/activation against the exact bundle digest.

Test classes: protocol/schema, mandatory port semantics, declared optional capabilities, effect idempotency/status/reconciliation claims, cancellation/timeouts, concurrency, crash/restart, fencing behavior, resource cleanup, sandbox/security boundary, snapshot/restore claims, and data consistency.

Changing source/dependencies changes bundle digest and invalidates prior conformance/approval for that executable content.
