> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Stable Port Catalog

Ports exist only at supported substitution boundaries. Every port has a semantic major version and binding scope.

## Catalog
- [KernelStorePort](kernel-store.md) — `bootstrap-global`
- [EventJournalPort](event-journal.md) — `generation-global`
- [MessageQueuePort](message-queue.md) — `generation-global`
- [SandboxPort](sandbox.md) — `run-scoped`
- [WorkspacePort](workspace.md) — `run-scoped`
- [ArtifactStorePort](artifact-store.md) — `run-scoped`
- [ModelPort](model.md) — `run-scoped`
- [MemoryStorePort](memory-store.md) — `run-scoped`
- [ContextPort](context.md) — `run-scoped`
- [ToolRuntimePort](tool-runtime.md) — `run-scoped`
- [SecretStorePort](secret-store.md) — `generation-global`
- [TransportPort](transport.md) — `generation-global`
- [AgentLoopPort](agent-loop.md) — `run-scoped`

## Rules

- Mandatory semantics are not negotiable.
- Stronger features are optional capabilities.
- Adapters cannot silently drop required semantics.
- Port operations that can produce effects integrate with the Effect Coordinator.
- Backend brand names never determine workflow semantics.
