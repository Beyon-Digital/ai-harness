> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Event Model and Catalog

Durable events have event ID/version, stream key/sequence, time, run/task/session/effect IDs as applicable, causation/correlation IDs, sensitivity, retention, and typed payload.

Families: runtime, graph, loop, effect/model/tool, workspace/resource, memory/context, security/approval/secret, adapter/config, recovery/daemon.

Canonical names live in `spec/events/catalog.yaml`.
