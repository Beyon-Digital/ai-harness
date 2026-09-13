> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Rust Crate Map

The MVP uses multiple crates for ownership and test boundaries while remaining one daemon.

| Crate | Owns | Must not own |
|---|---|---|
| `domain` | IDs, enums, immutable domain structs | DB/transport logic |
| `errors` | typed error codes/envelopes | component policy |
| `kernel-store` | `KernelStore` + transaction traits | SQLite details |
| `kernel-store-sqlite` | SQLite schema/repositories/transactions | runtime policy |
| `command-coordinator` | linearization/idempotency/atomic command orchestration | AI behavior |
| `events` | event envelope, sequence/cursor helpers, journal port, live bus | persistent backend |
| `event-journal` | re-export of the journal port owned by `events` | SQLite details |
| `event-journal-sqlite` | SQLite event projection | canonical runtime truth |
| `message-queue` | queue port + in-memory adapter | canonical runtime truth |
| `runtime` | run state machine, run fencing, recovery disposition | graph storage |
| `run-graph` | graph mutation/readiness/cancellation epoch logic | workflow AI policy |
| `effects` | effect contracts/state/leases/reconciliation coordinator | vendor-specific calls |
| `resources` | reservations, budgets, delegation | scheduler timing |
| `scheduler` | durable timers/claim/fire/cancel | workflow planning |
| `identity` | principal/actor/delegation types | permission policy |
| `permissions` | capability evaluation | secret storage |
| `approvals` | immutable approval request/response validation | UI |
| `secrets` | broker + secret-store interface | global env injection |
| `process-supervisor` | child lifecycle/private IPC/digest-bound spawn | adapter semantics |
| `adapter-registry` | manifests, registry, capability resolution | process ownership |
| `adapter-protocol` | framed protobuf external process protocol | business logic |
| `resource-uri` | URI parsing/resolution dispatch | physical storage |
| `workspace` | workspace coordinator/leases/local adapter | sandbox internals |
| `sandbox` | sandbox manager/T0 adapter/tier checks | generated behavior |
| `artifacts` | local artifact adapter | workspace mutation semantics |
| `config-engine` | config parsing/generations/profile resolution/activation | KernelStore replacement |
| `control-api` | gRPC API translation/auth context | direct DB writes |
| `observability` | tracing/metrics/audit helpers/classification | raw secret persistence |
| `testkit` | deterministic fixtures, clocks, crash/fault hooks | production behavior |
| `agentctl` | local CLI using public Control/Event APIs | direct DB access |
| `agentd` | composition root/startup/shutdown | component implementation details |

## Dependency direction

```text
agentd
  ↓
control-api / command-coordinator / config-engine / supervisors
  ↓
runtime / run-graph / effects / permissions / resources
  ↓
ports + domain + errors

SQLite/external implementations depend inward on port/domain contracts.
Core domain crates never depend on implementations.
```

Avoid cyclic crate dependencies; shared primitives move downward into `domain` or a narrowly named common crate only when necessary.
