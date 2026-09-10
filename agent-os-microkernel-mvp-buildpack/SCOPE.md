# MVP Scope

## In scope

### Trusted Rust kernel

- `agentd` lifecycle and drain-first shutdown.
- Canonical domain types and error model.
- SQLite `KernelStore` with required transactional semantics.
- OS single-instance lock + daemon fencing epoch.
- Command Coordinator and idempotency records.
- Transactional outbox.
- SQLite Event Journal and live subscriber bus.
- Runtime Manager.
- transactional RunGraph.
- Effect Coordinator, effect claims/fencing, reconciliation state.
- Resource reservations.
- durable scheduler/timers.
- principal/actor/delegation identity.
- capability engine.
- immutable approval requests.
- Secrets Broker with a test store and macOS Keychain-backed local store.
- Process Supervisor.
- Adapter Registry and capability negotiation.
- Config Engine and immutable config generations.
- ResolvedRunEnvironment freeze/persistence.
- Workspace Coordinator and local Git-worktree adapter.
- T0 local trusted sandbox.
- local artifact store.
- logical Resource URI resolver.
- local gRPC Control API over Unix-domain socket.
- structured logs/metrics hooks required to diagnose invariants.

### Fixture implementations

- fixture AgentLoop external process;
- fixture external effect adapter;
- fixture adapter manifest/bundle digest;
- deterministic test clock and ID provider inside testkit only.

## Out of scope

The MVP does not attempt the user-facing AI product yet. There is no production model loop, remote relay, phone UI, full memory engine, full tool ecosystem, generated extensions, or cloud execution.

The control plane must nevertheless expose stable seams needed by those future layers.

## Security boundary

T0 local-process execution exists only for trusted fixture/user-audited code. Requests for T2/T3 execution **must fail closed** when no qualifying sandbox adapter is installed.
