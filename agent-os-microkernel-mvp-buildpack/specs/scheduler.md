> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Scheduler

The MVP scheduler provides durable one-shot timers used by runtime waits, leases, and tests. Recurring user automations are out of scope.

## Timer state

```text
Scheduled -> Claimed -> Fired
Scheduled -> Cancelled
```

Claim and cancel both CAS from `Scheduled` at an expected version. Exactly one wins.

## Worker

A scheduler worker:

1. polls due `Scheduled` timers;
2. transactionally claims one with executor/fencing token and increments version;
3. emits/executes the corresponding kernel command;
4. transactionally marks `Fired` if still current.

A fired timer never mutates runtime state directly; it submits a normal internal kernel command.
