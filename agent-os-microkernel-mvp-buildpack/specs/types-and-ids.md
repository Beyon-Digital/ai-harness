> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Types and IDs

## ID format

Use UUIDv7 for persisted entity IDs because IDs are globally unique and approximately time-sortable without central coordination.

Strong Rust newtypes are required:

```rust
RunId(Uuid)
TaskId(Uuid)
SessionId(Uuid)
EffectId(Uuid)
EventId(Uuid)
WorkspaceId(Uuid)
LeaseId(Uuid)
ReservationId(Uuid)
TimerId(Uuid)
ConfigGenerationId(Uuid)
ApprovalRequestId(Uuid)
CapabilityGrantId(Uuid)
AdapterInstanceId(Uuid)
```

Do not pass raw `String` IDs through internal kernel APIs.

## Time

Persist wall-clock timestamps as UTC Unix milliseconds. Lease/timeout calculations use a monotonic process clock while running; persisted expiry uses wall-clock plus defensive recovery rules.

## Digests

Content/request digests use SHA-256 over canonical bytes. For JSON, canonicalize before hashing or hash the original immutable serialized request bytes stored with the record.

## Revisions

All revisions/epochs/sequences are unsigned 64-bit integers and monotonically increase within their scope. Overflow is treated as fatal invariant exhaustion rather than wrapping.
