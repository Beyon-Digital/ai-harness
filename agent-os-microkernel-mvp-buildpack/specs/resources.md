> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Resource Reservations and Budgets

Resources are accounted through durable reservations, not transient counters.

## Reservation states

`reserved -> allocated -> released`, with `expired` and `unknown` exceptional states.

## MVP budget types

- `child_run_slots`;
- `sandbox_slots`;
- `model_tokens` (structure only; no real model adapter yet);
- `model_cost_microunits`;
- `wall_clock_ms`;
- `disk_bytes`.

## Delegation

A child reservation may reference a parent reservation. Transactional checks enforce:

```text
sum(active child allocations/reservations) <= parent delegated amount
```

No child can reserve more authority/budget than its ancestry provides.

Reclamation of stale external allocations uses fencing/status where applicable; uncertain allocations become `unknown` rather than silently released.
