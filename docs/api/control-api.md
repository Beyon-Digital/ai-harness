> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Control API

Core resources: agents, sessions, tasks, runs, run graph, workspaces, artifacts, effects, extensions, adapters, config generations, approvals, and health.

State-changing commands carry authenticated principal/device, actor, idempotency key, expected revision when applicable, correlation ID, and request digest. Remote and local clients use the same logical command model even if transports differ.
