> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Effect Coordinator

The Effect Coordinator owns durable side-effect safety.

## Exposed functionality

- `prepare_effect`.
- `claim_effect`.
- `mark_dispatched`.
- `acknowledge_effect`.
- `commit_effect`.
- `fail_effect`.
- `mark_unknown`.
- `reconcile_effect`.
- `cancel_effect`.

## Claim/fencing

Claim atomically writes executor ID, lease expiration, and fencing token. Only the current fencing token may advance the effect. Expired workers may return data for diagnostics but cannot commit authoritative outcomes.

## Model calls

Model invocations are effect-tracked because duplicate retries can incur cost and produce a different output.

## Tool semantics

The coordinator computes an **effective** effect policy from port-known semantics, extension claim, trust tier, conformance evidence, and user/admin policy. Generated code cannot self-promote to “safe retry.”
