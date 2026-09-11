> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# KernelStore Specification

`KernelStore` is a trusted native port. The MVP implementation is SQLite.

## Trait shape

Use a transaction object rather than many unrelated repository calls that could accidentally commit separately.

```rust
#[async_trait]
pub trait KernelStore: Send + Sync {
    async fn begin_write(&self, ctx: TxContext) -> Result<Box<dyn KernelTxn + '_>>;
    async fn begin_read(&self) -> Result<Box<dyn KernelReadTxn + '_>>;
    async fn acquire_daemon_fence(&self, instance: DaemonInstanceId) -> Result<DaemonFence>;
}
```

`KernelTxn` exposes typed repository operations for runs/graph/effects/resources/events/idempotency/config/security. It exposes `commit(self)` and `rollback(self)`; it does **not** expose arbitrary SQL to callers outside the SQLite implementation.

## Inception schema and version rule

`specs/kernel-store-schema.sql` is the only authority that creates tables. It is executed verbatim when `kernel.db` is created and seeds `kernel_meta(schema_version) = '1'`; startup rejects any other schema version. Repositories never auto-create runtime tables. The schema also carries the normative PRAGMAs, all CHECK constraints, and the immutability triggers.

Table inventory:

| Group | Tables |
|---|---|
| Daemon/meta | `kernel_meta`, `daemon_fence` |
| Task/session/graph | `tasks`, `sessions`, `runs`, `run_graph_heads`, `run_dependencies` |
| Environment/bindings | `resolved_run_environments`, `resolved_bindings` |
| Agent specs | `agent_specs` |
| Workspace | `workspaces`, `workspace_leases` |
| Effects/resources/timers | `effects`, `resource_reservations`, `timers` |
| Loop/decisions | `loop_turns`, `decisions` |
| Security/approvals | `capability_grants`, `delegation_hops`, `approval_requests`, `approval_responses` |
| Adapters | `adapter_registrations`, `adapter_instances`, `conformance_reports` |
| Artifacts | `artifacts` |
| Config | `config_generations`, `active_config_generation` |
| Idempotency/events | `idempotency_records`, `event_stream_heads`, `outbox_events` |

The durable Event Journal is a separate database (`events.db`) with its own schema, `specs/event-journal-schema.sql`, extracted from `event-pipeline.md`; it shares no tables with `kernel.db`.

Persisted TEXT state literals for the GC-3 tables (identical to the CHECK constraints in the schema):

| Column | Persisted literals |
|---|---|
| `loop_turns.state` (specs/runtime-manager.md) | `issued`, `accepted`, `stale` |
| `decisions.decision_type` (contracts/protocols/agent_loop.proto) | `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval` |
| `adapter_instances.state` (specs/process-supervisor.md) | `starting`, `ready`, `exited`, `failed` |
| `conformance_reports.result` (specs/adapter-registry.md) | `pass`, `fail` |

## Mandatory invariants

- Every write transaction asserts current daemon fencing epoch.
- Idempotency record is inserted/validated inside the same command transaction.
- Event sequences are allocated by atomically updating `event_stream_heads`.
- Outbox event insertion uses unique `event_id` and `(stream_key, sequence)`.
- `ResolvedRunEnvironment` and resolved bindings are immutable after run start.
- Effect state transitions check expected state and fencing token.
- Workspace lease transfer checks expected lease epoch.
- Timer state changes check expected version/state.

## SQLite implementation

Use a bounded pool for reads, but serialize correctness-critical writers through SQLite transactions rather than application-level mutex as the source of truth. Application-level queues may reduce contention but cannot replace DB constraints/CAS.
