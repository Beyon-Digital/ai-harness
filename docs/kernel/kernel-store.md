> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Kernel Metadata Store

`KernelStorePort` is stronger than a generic key/value store and is bootstrap-global.

## Stores

- AgentSpec metadata/revisions.
- Tasks, runs, sessions.
- RunGraph edges/revisions/cancellation epochs.
- `ResolvedRunEnvironment`.
- Effect records and executor leases/fences.
- Resource reservations.
- Idempotency records.
- Recovery dispositions.
- Adapter/extension registrations.
- Config generations/active pointer.
- Approval/grant records.
- Daemon lease/fencing epoch.
- Event stream sequence allocations + outbox entries.

## Mandatory backend semantics

ACID transactions, conditional writes/CAS, uniqueness, monotonic per-stream sequence allocation, durable commits, lease/fencing, and transactional outbox. Optional capabilities may include online backup, replication, or read replicas but cannot weaken base semantics.

## Extension policy

Because this port participates directly in kernel atomicity/fencing invariants, initial KernelStore adapters are **trusted Rust/native implementations only** and are changed only under maintenance mode. Agent-generated or arbitrary process adapters cannot implement the active KernelStore in the initial implementation.
