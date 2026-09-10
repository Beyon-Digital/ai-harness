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
