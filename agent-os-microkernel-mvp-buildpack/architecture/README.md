> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# MVP Architecture

```text
                         Local CLI / test client
                                  │
                                  ▼
                    gRPC over Unix domain socket
                                  │
                    ┌─────────────▼─────────────┐
                    │          agentd           │
                    │          Rust             │
                    │                           │
                    │  Control API              │
                    │      │                    │
                    │  Command Coordinator      │
                    │      │                    │
                    │  ┌───▼────────────────┐   │
                    │  │ Kernel mechanisms  │   │
                    │  │ Runtime Manager    │   │
                    │  │ RunGraph           │   │
                    │  │ Effect Coordinator │   │
                    │  │ Resources/Scheduler│   │
                    │  │ Permissions/Secrets│   │
                    │  │ Adapter Registry   │   │
                    │  │ Workspace/Sandbox  │   │
                    │  └───┬────────────────┘   │
                    │      │                    │
                    │  KernelStorePort          │
                    │      │                    │
                    │  SQLite Kernel DB         │
                    │  canonical state+outbox   │
                    │      │                    │
                    │  Outbox Dispatcher        │
                    │     / \                   │
                    │    ▼   ▼                  │
                    │ Event  Live               │
                    │Journal Bus                │
                    └────┬──────────────────────┘
                         │ private inherited IPC
               ┌─────────┴─────────┐
               ▼                   ▼
        Fixture AgentLoop    Fixture Effect Adapter
        external process    external process
```

## MVP data plane

The daemon is one process. Internal crates communicate through Rust calls/channels, not sockets. Only external adapters/loops are separate processes.

## Canonical commit path

```text
Control command / accepted loop decision
             │
             ▼
Command Coordinator
             │
             ▼
BEGIN KernelStore transaction
  validate fences/revisions/idempotency
  mutate canonical rows
  mutate RunGraph
  reserve resources if needed
  create EffectRecord if needed
  allocate event sequence(s)
  insert outbox event(s)
  persist idempotency outcome
COMMIT
             │
             ▼
return durable command result
             │
             └────► async outbox publisher
```

No other component may acknowledge an authoritative state mutation that did not pass through this boundary.
