> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Event Streaming API

Subscribe by authorized session/task/run/effect/event family and resume from an opaque cursor scoped to that subscription.

Durable stream responses expose retention-gap errors when the requested cursor is no longer available. Clients then re-query canonical state and continue from an allowed checkpoint rather than assuming missing events.
