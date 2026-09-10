> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Adapter Capability Negotiation

A run/service declares required and preferred semantic capabilities. Resolver filters compatible port major version, mandatory capabilities, trust/environment constraints, binding scope, health, and config pin/priority.

Example: a workflow requiring queue replay cannot bind an SQS adapter that advertises `replay=false`; activation/run creation fails before execution rather than silently degrading.

Negotiation result is persisted in `ResolvedRunEnvironment` for run-scoped ports or active generation metadata for daemon-wide services.
