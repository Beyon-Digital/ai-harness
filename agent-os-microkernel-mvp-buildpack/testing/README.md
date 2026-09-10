> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Test Strategy

The MVP is correctness-heavy. Tests are part of the architecture, not a final polish phase.

## Layers

1. **Pure unit tests:** state machines, capability subset logic, effect policy merge, URI parser.
2. **Store contract tests:** every KernelStore transaction invariant against SQLite.
3. **Concurrency tests:** race graph edges, claims, timers, workspace leases, effect fencing.
4. **Protocol tests:** malformed/forged adapter handshake, duplicate frames, cancellation/deadline.
5. **Crash/fault tests:** inject failure after each durable boundary.
6. **Integration tests:** real daemon + SQLite files + fixture child processes + UDS client.
7. **Property tests:** arbitrary state transition sequences cannot reach illegal states/invariants.

## Required tooling

- `cargo test --workspace`.
- `proptest` for state/graph/effect properties.
- Tokio paused/test time where practical.
- temp directories for isolated DB/socket/workspace fixtures.
- process-level integration tests for crash/restart.
