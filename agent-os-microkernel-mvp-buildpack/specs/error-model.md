> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Error Model

Every public/internal error maps to a stable code plus structured context.

```rust
struct AgentOsError {
    code: ErrorCode,
    message: String,
    retry: RetryDisposition,
    correlation_id: Option<CorrelationId>,
    details: BTreeMap<String, String>,
}

enum RetryDisposition { Never, SafeSameOperation, Conditional }
```

Required error families:

- `VALIDATION_*`
- `AUTHENTICATION_*`
- `AUTHORIZATION_*`
- `APPROVAL_REQUIRED`
- `IDEMPOTENCY_CONFLICT`
- `REVISION_CONFLICT`
- `STALE_LOOP_DECISION`
- `FENCING_REJECTED`
- `CAPABILITY_UNSUPPORTED`
- `GRAPH_CYCLE`
- `GRAPH_STATE_INVALID`
- `EFFECT_UNKNOWN`
- `EFFECT_NOT_RETRYABLE`
- `RESOURCE_EXHAUSTED`
- `WORKSPACE_LEASE_CONFLICT`
- `ADAPTER_IDENTITY_MISMATCH`
- `ADAPTER_PROTOCOL_MISMATCH`
- `STORE_BUSY`
- `STORE_INVARIANT`
- `DAEMON_RESTART_REQUIRED`
- `NOT_FOUND`
- `CANCELLED`
- `DEADLINE_EXCEEDED`
- `INTERNAL_INVARIANT`

Never encode retry safety only in a human message.
