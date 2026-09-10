> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Planning and Delegation

Planning is behavioral policy. A Hermes loop, Codex loop, planner/executor loop, or generated loop can create completely different orchestration topologies while using the same RunGraph substrate.

Child creation specifies AgentSpec/loop/profile requirements, delegated capabilities/budget, workspace authority mode, and parent cancellation epoch. The kernel decides whether the delegation is legal and creates the child atomically.
