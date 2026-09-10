> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Persistence and Atomic Commit Model

## Single authority

The **Kernel Metadata Store** is authoritative for execution correctness. It stores runs/tasks/sessions, graph edges, run revisions, recovery dispositions, effect records, resource reservations, idempotency records, adapter/config registrations, approvals/grants, daemon fencing state, and transactional outbox entries.

The Event Journal is a durable historical projection, not a co-authoritative state database.

## Atomic command transaction

A state-changing command commits all applicable records together:

```text
canonical state mutation
+ RunGraph mutation
+ resource reservation
+ idempotency outcome
+ effect preparation
+ stream sequence allocation
+ outbox event(s)
= ONE KernelStore transaction
```

If the transaction does not commit, none of these facts are externally acknowledged as authoritative.

## Outbox publishing

After commit, an outbox worker publishes durable events to the Event Journal and live bus. Re-publication is safe because every event has a stable `event_id` and stream sequence.

## Required KernelStore semantics

ACID transactions, conditional mutation/CAS, unique constraints, monotonic per-stream sequence allocation, durable commit, lease/fencing primitives, and transactional outbox are mandatory. A backend that cannot provide the contract is not a valid KernelStore adapter.
