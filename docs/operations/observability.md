> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Observability

Structured logs/metrics/traces carry run/task/session/effect/adapter IDs and correlation/causation identifiers.

Metrics include active runs/sandboxes/effects, unknown effects, reconciliation outcomes, model latency/cost/tokens, tool failures, adapter restarts, queue depth, store latency, reservation usage, dropped ephemeral events, and lagging/disconnected durable subscribers.

All output follows sensitivity/retention projection rules.
