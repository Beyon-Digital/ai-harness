> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Extension SDK

Rust reference types plus generated Python/TypeScript SDKs expose handshake, execution context, resource URIs, cancellation/deadlines, structured errors, logs/metrics with sensitivity labels, kernel intents, tool/adapter registration, effect operation IDs/status, and conformance-test helpers.

The SDK deliberately does not expose raw authoritative process spawning, graph mutation, global secret access, or direct active-config mutation.
