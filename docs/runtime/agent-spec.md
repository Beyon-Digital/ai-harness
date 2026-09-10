> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# AgentSpec

Persistent versioned agent definition: instructions, default loop, runtime profile, context/memory strategies, model router, tools/skills, default permissions, workspace policy, and resource defaults.

A run records the exact AgentSpec version/digest it resolved. Editing an AgentSpec never mutates an active run.
