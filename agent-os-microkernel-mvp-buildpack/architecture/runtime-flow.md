> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Runtime and Control Flow

## Run start

1. Client submits `CreateTaskRun` command with idempotency key.
2. Command Coordinator validates principal/actor and prior idempotency outcome.
3. Config Engine resolves the active profile and exact run-scoped adapters.
4. Permission Engine computes initial grants/delegation constraints.
5. Workspace Coordinator creates/acquires workspace lease if requested.
6. `ResolvedRunEnvironment` is materialized and persisted.
7. Task + Run + graph root + reservations + events commit atomically.
8. Runtime marks the run `Ready`; a claim transitions it into `Running`.
9. Loop Supervisor issues a fenced `LoopInput` to the resolved loop process.

## Loop turn

A loop cannot execute privileged mechanisms. It returns a `LoopDecision` bound to:

- run ID;
- run revision;
- loop epoch;
- step sequence;
- input event cursor;
- turn ID;
- decision ID.

The Runtime Manager/Command Coordinator accepts it only if all expected values still match. Acceptance and resulting canonical mutation/effect/child creation/cursor advancement occur atomically.

## Child run

Parent loop requests a child. Kernel verifies:

- current parent cancellation epoch;
- `agent.spawn` capability;
- delegated budget/capabilities are subsets;
- workspace delegation mode is valid;
- resolved profile/adapters are available.

Child creation + graph edge + delegated grants + workspace lease/fork + events are one transaction where possible. Physical workspace creation may be coordinated through a prepared effect/resource record if it is externally effectful.
