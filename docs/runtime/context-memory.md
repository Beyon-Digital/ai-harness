> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Context and Memory

## Separation

`MemoryStorePort` handles normalized persistence/search. `MemoryStrategy` decides what to remember/forget/consolidate/recall. Context strategy builds model-ready context from session, workspace, artifacts, memory, tools, and other sources under a budget.

## Memory provenance

Every persistent memory records creator agent/run, source references, strategy ID/version, timestamp, sensitivity, trust level, namespace, and optional verification metadata.

## Namespace authority

Writes are separately authorized for session, agent-private, shared semantic, user identity/preferences, and procedural/skill namespaces. A research child with generic memory access does not automatically gain authority to rewrite global identity/procedural memory.
