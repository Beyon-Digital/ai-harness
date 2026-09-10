# Personal AI Agent OS — Canonical Architecture & Implementation Specification

This repository is the **first canonical inception specification** for a local-first, remotely accessible, extensible AI-agent operating system. It defines the architecture to build from inception.

The runtime is centered on a trusted Rust daemon, `agentd`. The kernel owns deterministic mechanisms, execution truth, recovery, and security invariants. Replaceable adapters implement stable infrastructure ports. Agent loops and behavioral services remain swappable per run.

## Core architecture

- A transactional **Kernel Metadata Store** is the authoritative source of runtime truth.
- Durable events leave the Kernel Store through a **transactional outbox**; the Event Journal is downstream and not co-authoritative.
- External/model/tool mutations are tracked by durable **EffectRecord** state with operation IDs, executor leases/fencing, reconciliation, and explicit `Unknown` handling.
- Loop decisions are fenced by run revision, loop epoch, step sequence, input cursor, and decision ID.
- RunGraph mutations and ready-run claims are transactional.
- Every running `AgentRun` persists immutable **ResolvedRunEnvironment** bindings for audit and reproducibility.
- Infrastructure bindings have explicit scopes: bootstrap-global, generation-global, and run-scoped.
- Untrusted/generated code requires enforceable sandbox security tiers; trusted local-process execution is not a security boundary.
- Workspace write authority supports delegated transfer, isolated parallel forks, and capability-negotiated coordinated shared writes.
- Persistent memory carries provenance, sensitivity, trust, and namespace-specific write authority.
- Extension bundles are content-addressed and dependency-locked; approvals bind to immutable digests.
- Adapter identity is established by the supervisor/kernel rather than self-asserted.
- Remote approvals bind to exact immutable requests.
- Logs/events are data-classified; durable-stream overflow cannot silently lose history.
- Historical replay means state/history reconstruction plus reproducibility metadata, not deterministic re-execution of nondeterministic external systems.

## Start here

- [Documentation home](docs/index.md)
- [Architecture overview](docs/architecture/README.md)
- [Persistence and atomicity model](docs/architecture/persistence-model.md)
- [Effect model](docs/architecture/effect-model.md)
- [RunGraph](docs/architecture/run-graph.md)
- [Binding scopes and immutable run bindings](docs/architecture/binding-scopes.md)
- [Rust kernel](docs/kernel/README.md)
- [Port catalog](docs/ports/README.md)
- [Runtime profiles](docs/configuration/profiles.md)
- [Security model](docs/security/README.md)
- [Canonical machine-readable specs](spec/README.md)
- [Complete implementation plan](PLAN.md)

## Authority hierarchy

1. `spec/` — normative machine-readable contracts, IDs, schemas, and protocol fields.
2. `docs/` — human-readable architecture, semantics, implementation guidance, and examples.
3. Generated SDKs/reference material must be derived from `spec/` and checked for freshness in CI.

There is intentionally **one** human-authored documentation tree and one canonical machine-readable specification tree.
