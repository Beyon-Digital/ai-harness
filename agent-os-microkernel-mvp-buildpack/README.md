# Agent OS Microkernel + Base Control Plane — MVP Build Pack

This folder is an implementation-ready build specification for the **first working microkernel and local base control plane** of the Agent OS.

It is intentionally narrower than the full product architecture. The goal is to build the trusted Rust substrate correctly before adding real Hermes/Codex loops, model providers, memory systems, remote phone access, or agent-generated extensions.

## MVP outcome

A local `agentd` daemon on macOS that can:

- own authoritative runtime state in a transactional SQLite-backed `KernelStore`;
- enforce single-writer daemon fencing;
- process idempotent commands through one command coordinator;
- atomically commit canonical state + RunGraph changes + reservations + prepared effects + outbox events;
- publish durable events through Event Journal → Live Bus ordering;
- manage Tasks, Sessions, AgentRuns, RunGraph dependencies, child runs, cancellation epochs, and recovery dispositions;
- durably coordinate external side effects with effect leases/fencing and `Unknown` recovery semantics;
- account for resource reservations and durable timers;
- enforce principals, actors, delegation chains, capabilities, approvals, and secret mediation;
- supervise external process adapters over private inherited IPC;
- register/version/verify adapters and negotiate port capabilities;
- manage local workspaces, workspace leases, isolated forks, and explicit write-authority transfer;
- expose a local Control API over a Unix-domain socket;
- persist immutable `ResolvedRunEnvironment` bindings for every started run;
- exercise the system end-to-end with a fixture external AgentLoop and fixture effect adapter.

## Explicitly out of scope for this build pack

- Production Hermes/Codex loop implementations.
- Real OpenAI/Anthropic/model integrations.
- Memory strategies/context engines/RAG.
- Remote cloud relay, mobile app, or web app.
- Agent-authored extension generation.
- Cloud infrastructure adapters.
- T2/T3 production sandbox implementation. The kernel must **enforce** the tier contract and refuse untrusted execution when no qualifying adapter exists.
- Any lifecycle for changing an already-deployed database format. This pack defines only the inception schema.

## Authority

1. The architectural decisions in this pack are frozen for the MVP.
2. `/contracts` is the build-pack snapshot of the canonical inception contracts.
3. `/specs` defines implementation semantics where the canonical contracts were intentionally abstract.
4. `/tasks` is the executable work breakdown for coding agents.
5. `dag.yaml` is the machine-readable dependency graph.

## Start here

- [AI coding-agent instructions](AGENTS.md)
- [MVP scope and non-goals](SCOPE.md)
- [Frozen decisions](DECISIONS.md)
- [Source contract corrections](SOURCE_CORRECTIONS.md)
- [Architecture](architecture/README.md)
- [Crate map](architecture/crate-map.md)
- [Implementation plan](PLAN.md)
- [Task DAG](DAG.md) — 59 executable tasks
- [Machine-readable DAG](dag.yaml)
- [Component specifications](specs/README.md)
- [Test strategy](testing/README.md)
- [MVP release gate](MVP_EXIT_CRITERIA.md)

## Repository this pack expects the coding agent to create

```text
agent-os/
├── Cargo.toml
├── rust-toolchain.toml
├── crates/
│   ├── agentd/
│   ├── domain/
│   ├── errors/
│   ├── kernel-store/
│   ├── kernel-store-sqlite/
│   ├── command-coordinator/
│   ├── events/
│   ├── event-journal/
│   ├── event-journal-sqlite/
│   ├── message-queue/
│   ├── runtime/
│   ├── run-graph/
│   ├── effects/
│   ├── resources/
│   ├── scheduler/
│   ├── identity/
│   ├── permissions/
│   ├── approvals/
│   ├── secrets/
│   ├── process-supervisor/
│   ├── adapter-registry/
│   ├── adapter-protocol/
│   ├── resource-uri/
│   ├── workspace/
│   ├── sandbox/
│   ├── artifacts/
│   ├── config-engine/
│   ├── control-api/
│   ├── observability/
│   ├── testkit/
│   └── agentctl/
├── proto/
├── schema/
├── config/
├── fixtures/
└── tests/
```

The crate split is for source-code ownership and testability, **not** for independent services. The MVP remains one daemon plus supervised external fixture processes.
