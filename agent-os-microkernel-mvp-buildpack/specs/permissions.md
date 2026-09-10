> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Permission Engine

## Decision

```rust
enum PermissionDecision {
  Allow { grant_refs: Vec<GrantId> },
  Deny { reason: DenyReason },
  RequireApproval { request: ApprovalRequestDraft },
}
```

## Capability families in MVP

- workspace read/write/fork/merge/transfer;
- network connect (structure/enforcement metadata; T0 does not claim network isolation);
- secret use/sign-or-act;
- agent spawn;
- extension register/enable/disable;
- config propose/test/activate/rollback;
- effect reconcile/resolve-unknown;
- resource reserve.

## Joint evaluation

Secret authority and egress authority are evaluated together. A grant to use a production secret plus unrestricted network access is a materially broader permission and may require explicit approval even when each individual grant exists.
