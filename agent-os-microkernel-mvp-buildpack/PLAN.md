# Microkernel + Base Control Plane MVP Implementation Plan

> [Home](README.md) · [DAG](DAG.md) · [Tasks](tasks/README.md) · [Specs](specs/README.md)

## Build strategy

Implement correctness primitives bottom-up, then compose them through one end-to-end fixture path. Do not begin production AI integrations until the release gate passes.

## Stage A — Foundation

- Rust workspace/toolchain/CI quality gates.
- Contract snapshot + protobuf generation.
- domain IDs/types/errors.
- testkit, deterministic clock/fault hooks, tracing bootstrap.

## Stage B — Authoritative persistence

- inception SQLite schema.
- KernelStore transaction abstraction.
- SQLite implementation.
- OS writer lock + daemon fencing.
- Command Coordinator/idempotency/outbox transaction.

## Stage C — Events

- event envelope/stream sequencing.
- SQLite Event Journal.
- outbox dispatcher.
- Live Bus and cursor/lag semantics.

## Stage D — Runtime and graph

- run state machine/recovery dispositions.
- RunGraph transactions/cycle checks.
- ready claims/cancellation epochs.
- immutable run binding scaffolding.

## Stage E — Effects/resources/timers

- effect contract resolution.
- durable EffectRecord transitions.
- executor lease/fencing.
- reconciliation/Unknown path.
- resource reservations/budget delegation.
- CAS timers.

## Stage F — Security/control mechanisms

- principal/actor/delegation chain.
- capability engine.
- immutable approvals.
- Secrets Broker and local Keychain adapter.

## Stage G — External process/adapter substrate

- private socketpair supervisor.
- external protocol framing/handshake.
- adapter registry/content digest/capability negotiation.
- fixture adapter.

## Stage H — Resource execution substrate

- Resource URI resolver.
- Workspace Coordinator/local worktree adapter.
- workspace transfer/fork/merge.
- T0 trusted sandbox + tier rejection.
- local artifact store.

## Stage I — Config + frozen bindings

- inception config schema.
- immutable generations/activation tests.
- profile resolver.
- persist complete `ResolvedRunEnvironment` before execution.

## Stage J — Base Control API + fixture loop

- gRPC UDS server.
- command/query/event surfaces.
- CLI.
- external fixture AgentLoop using fenced loop protocol.

## Stage K — System verification

- end-to-end parent/child/effect workflow.
- crash points around every persistence/effect boundary.
- concurrency/property tests.
- security/invariant tests.
- release-gate script.

The exact task dependency graph is in `dag.yaml`; task IDs in `tasks/` are the executable units of work.
