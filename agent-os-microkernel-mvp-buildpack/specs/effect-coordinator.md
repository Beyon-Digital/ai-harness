> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Effect Coordinator

## Effective contract

Effect semantics use independent axes:

```text
EffectClass: EffectClassReadOnly | LocalMutation | ExternalMutation | Opaque
Idempotency: NaturallyIdempotent | IdempotencyKeySupported | NotIdempotent | Unknown
Reconciliation: StatusLookup | ResultLookup | DeterministicInspection | Impossible | Unknown
Cancellation: BeforeDispatch | Cooperative | ProviderSpecific | Unsupported
Compensation: optional capability only
```

An external/generated tool's declaration is a claim. Effective policy is the conservative intersection of:

- kernel-known port semantics;
- adapter/extension declaration;
- trust tier;
- conformance evidence;
- user/admin policy.

Unknown tools default to `Opaque + Unknown + Unknown`.

## Lifecycle

```text
Prepared -> Claimed -> Dispatched -> Acknowledged -> Committed
                       |               |
                       +-> Failed      +-> Failed
                       +-> Cancelled
                       +-> Unknown
```

## Preparation

`prepare_effect` runs inside the command/loop-decision transaction and persists:

- stable `effect_id` and `operation_id` (same ID may be used initially);
- immutable request bytes + SHA-256 hash;
- effective contract;
- exact adapter ID/version/digest from frozen run environment;
- initial `Prepared` state;
- outbox event.

## Claim

Atomic conditional transition `Prepared|expired Claimed -> Claimed` sets:

- executor ID;
- monotonically increasing effect fencing token;
- lease expiry.

Only the current fencing token can advance authoritative state.

## Dispatch/recovery

Before calling adapter, persist `Dispatched`. If response is lost:

1. if reconciliation supported, query status/result using the same operation ID;
2. if safely idempotent and policy permits, redispatch same operation ID;
3. otherwise mark `Unknown` and set owning run recovery disposition to `BlockedUnknownEffect` or configured terminal action.

Never create a fresh operation ID for a retry of the same logical effect.
