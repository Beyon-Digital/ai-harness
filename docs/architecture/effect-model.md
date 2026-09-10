> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Effect Model and Side-Effect Safety

Every externally visible or potentially costly mutation is coordinated through a durable `EffectRecord`.

## Why

The kernel must distinguish:
- operation definitely never dispatched,
- operation dispatched and acknowledged,
- operation may have happened but acknowledgement was lost.

The final case is `Unknown`, not ordinary failure.

## Effect semantics

Effect behavior is described on independent axes:

```text
EffectClass: ReadOnly | LocalMutation | ExternalMutation | Opaque
Idempotency: NaturallyIdempotent | IdempotencyKeySupported | NotIdempotent | Unknown
Reconciliation: StatusLookup | ResultLookup | DeterministicInspection | Impossible | Unknown
Cancellation: BeforeDispatch | Cooperative | ProviderSpecific | Unsupported
Compensation: Optional capability; never assumed equivalent to rollback
```

## Lifecycle

```text
Prepared → Claimed → Dispatched → Acknowledged → Committed
                     ├────────────→ Failed
                     ├────────────→ Cancelled
                     └────────────→ Unknown
```

Claims carry executor identity, lease expiry, and fencing token. A stale executor cannot commit after losing the lease/fence.

## Adapter API

Effectful operations use stable `operation_id` and request hash. Where supported, adapters expose `status(operation_id)`, `reconcile(operation_id)`, and `cancel(operation_id)`.

## Safe retry rule

- If status/reconciliation establishes the outcome, commit that outcome.
- If the operation is safely idempotent, the same `operation_id` may be redispatched under policy.
- If neither is possible, transition to `Unknown` and require configured fail/ask-user/explicit-duplicate-risk policy.
- `Unknown` never means automatic retry.

## Tool claims are not trusted by default

Generated/unknown tools default to `Opaque + Unknown idempotency + Unknown reconciliation`. A stronger effective policy requires trusted built-in semantics, reviewed metadata, conformance evidence, or explicit user/admin policy.
