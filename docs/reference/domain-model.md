> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Canonical Domain Model

Core entities: AgentSpec, Session, Task, AgentRun, ResolvedRunEnvironment, RunGraphEdge, WorkspaceLease, Artifact, EffectRecord, ResourceReservation, MemoryRecord, CapabilityGrant, ApprovalRequest, ExtensionBundle, AdapterRegistration, ConfigGeneration, OutboxEvent, EventEnvelope, DaemonLease.

Normative wire/domain IDs live under `spec/domain/`.
