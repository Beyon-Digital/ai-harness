# Complete Implementation Plan

> **Navigation:** [Repository Home](README.md) · [Docs](docs/index.md) · [Canonical Spec](spec/README.md)

## Goal

Build a local-first Agent OS whose authoritative runtime is a reliable Rust daemon (`agentd`) on the user's Mac. It supports multiple concurrent agent loops, local/cloud execution, replaceable infrastructure adapters, agent-generated extensions, and secure remote control from phone/web clients.

## Frozen architectural decisions

- `agentd` is the trusted control plane.
- The kernel owns invariants, not AI policy.
- `KernelStorePort` is bootstrap-global and requires ACID transactions, CAS/conditional mutation, unique constraints, monotonic stream sequencing, transactional outbox, and writer fencing.
- Event Journal is downstream of the transactional outbox and is not the authority for current runtime state.
- All external side effects use durable `EffectRecord`s and stable operation IDs.
- `Unknown` non-reconcilable effects are never blindly retried.
- Model calls are effect-tracked because they cost money and are nondeterministic.
- Running runs pin immutable resolved bindings until completion; ordinary config activation never hot-swaps a live run.
- Parallel write-capable coding children default to isolated workspace forks/worktrees.
- Exclusive workspace write authority can be transferred to a child; shared concurrent writes require an adapter capability.
- Agent-generated/untrusted code cannot use the trusted local-process sandbox tier.
- Extension approval and conformance bind to content/dependency digests.
- The canonical contract source is `spec/`.

## Phase 0 — Canonical spec foundation

Deliver:
- Canonical domain, event, port, extension, config, and protocol specs under `spec/`.
- Stable IDs for ports/events/effect semantics.
- Contract versioning rules for public schemas and ports.
- Generated Rust/Python/TypeScript type pipeline design.
- CI checks for schema validity, contract consistency, generated-file freshness, and docs links.

Exit criteria:
- No duplicate port IDs or contradictory schemas.
- A change to a normative contract has one authoritative diff.

## Phase 1 — Rust daemon + KernelStore

Deliver:
- Rust workspace and `agentd` lifecycle.
- `KernelStorePort` contract and SQLite implementation.
- Daemon single-writer OS lock plus durable daemon lease/fencing epoch.
- Core domain types and transactional command coordinator.
- Transactional outbox tables/records.

Exit criteria:
- Mutating command can atomically persist canonical state + idempotency result + outbox events.
- Second daemon instance cannot become an active writer without a higher fencing epoch.

## Phase 2 — Event pipeline

Deliver:
- Live in-process bus.
- Outbox publisher.
- Event Journal port + SQLite implementation.
- Per-stream monotonic sequence and expected-position append.
- Cursor/reconnect semantics and retention-gap errors.
- Slow-subscriber handling.

Exit criteria:
- Durable event history cannot be silently lost because a client is slow.
- Restarted publisher can safely re-publish the same outbox event by `event_id`.

## Phase 3 — Runtime state machine + RunGraph

Deliver:
- `Task`, `AgentRun`, `Session`, `RunGraph` entities.
- Run revisions and loop epochs.
- Transactional parent-child/dependency mutation.
- Atomic ready-run claim.
- Cancellation epochs and descendant propagation.
- Persisted `RecoveryDisposition`.

Exit criteria:
- Concurrent graph mutations cannot introduce an undetected cycle.
- A child cannot escape a racing parent cancellation.
- Stale loop decisions fail CAS.

## Phase 4 — Effect Coordinator

Deliver:
- `EffectRecord` state machine.
- Effect executor leases/fencing.
- Operation IDs and request hashes.
- Effect contract resolution: class, idempotency, reconciliation, cancellation, compensation.
- Reconciliation/status API.
- `Unknown` recovery path and human-policy hooks.

Exit criteria:
- Crash after external execution but before acknowledgement does not cause blind retry.
- Two workers cannot concurrently dispatch the same claimed effect under valid fencing.

## Phase 5 — Resource accounting + scheduler

Deliver:
- Durable `ResourceReservation` identity/state.
- Budget inheritance/delegation.
- CAS timer states: Scheduled → Claimed/Fired or Cancelled.
- Recovery/reclamation of stale reservations.

## Phase 6 — Port framework and adapter registry

Deliver:
- Port version negotiation.
- Adapter manifests.
- Required/optional capability negotiation.
- Process adapter handshake using inherited private channel or authenticated per-instance endpoint.
- Adapter executable/bundle digest verification.
- Port conformance harness.

Exit criteria:
- Adapter identity is established by the supervisor/kernel, not merely by adapter claims.
- Resolver rejects adapters missing required semantics.

## Phase 7 — Sandbox + workspace + artifacts

Deliver:
- Sandbox trust tiers T0–T3.
- Local trusted process sandbox (T0 only).
- Strong container/VM sandbox for T2/T3.
- Workspace modes: READ_ONLY, EXCLUSIVE_WRITE, ISOLATED_FORK, SHARED_COORDINATED_WRITE.
- Delegatable `WorkspaceLease` and transfer/fork/merge APIs.
- Git worktree implementation.
- Artifact store implementation and logical resource URIs.

Exit criteria:
- Parallel coding children fork from the same parent scope and can explicitly merge useful output.
- Untrusted generated code cannot execute in a non-security-boundary sandbox.

## Phase 8 — Permissions, delegation, secrets, approvals

Deliver:
- Principal/actor/run/delegation-chain model.
- Capability subset delegation.
- Joint secret+egress policy evaluation.
- Brokered `SignOrAct`/short-lived credential support.
- Immutable approval request digest model.
- Device-authenticated approval responses.

## Phase 9 — AgentLoop runtime

Deliver:
- Loop protocol with `run_revision`, `loop_epoch`, `step_sequence`, `input_cursor`, `turn_id`, `decision_id`.
- CAS acceptance and atomic cursor advancement + effect/child creation.
- Process SDKs for Python/TypeScript.
- Minimal ReAct reference loop.
- Hermes-compatible and Codex-style loop adapters.

Exit criteria:
- Hermes and Codex loops run concurrently in one daemon.
- Delayed loop response from an older revision is rejected deterministically.

## Phase 10 — Models, memory, context, tools

Deliver:
- Model calls routed through Effect Coordinator.
- `MemoryStorePort` plus `MemoryStrategy` separation.
- Memory provenance/trust/sensitivity/namespace authorities.
- Context strategy/service interface.
- Tool effect claims resolved into kernel effective policy.
- Unknown/generated tool defaults to opaque/unknown side-effect semantics until trusted by policy/conformance.

## Phase 11 — Immutable resolved run environment

Deliver:
Persist at run start:
- AgentSpec version/digest.
- AgentLoop version/bundle digest.
- Runtime profile generation.
- Exact run-scoped adapter IDs/versions/digests/capabilities.
- Workspace base snapshot/commit, access mode, lease/delegation chain.
- Model provider/model ID/parameters.
- Context/memory/model-router versions.
- Tool/skill bundle digests.
- Security grants/approval references/delegation chain.
- Kernel version and protocol versions.

Exit criteria:
- Historical run can be audited without querying mutable “current config.”
- Re-run/test tooling can reconstruct the same declared logical environment where dependencies remain available.

## Phase 12 — Transactional config generations

Deliver:
- Runtime profiles for run-scoped bindings.
- Generation-global service configuration.
- Immutable config generations.
- CI-like validate → negotiate → sandbox-test → smoke → activate → health → rollback flow.
- Existing runs remain pinned to their original resolved bindings.
- Bootstrap-global bindings are established by daemon deployment configuration and are not mutable through ordinary runtime config transactions.

## Phase 13 — Content-addressed extension system

Deliver:
- Immutable extension bundles with dependency locks/inventory.
- Kernel-computed bundle digest.
- Approval/conformance/config binding to digest.
- Sandboxed build/test.
- Install/enable/disable/rollback by exact bundle identity.

## Phase 14 — Remote access and clients

Deliver:
- Local Control API.
- Mac outbound relay/tunnel connection.
- Device registration/revocation.
- Request-bound remote approvals.
- iOS/web/CLI clients.
- Minimal relay metadata and classified/redacted event projections.

## Phase 15 — Observability, backup, recovery

Deliver:
- Data-classified logs/events.
- Metrics/traces/audit.
- Crash recovery and effect reconciliation.
- Backup/restore.
- Drain-first shutdown sequence.
- Historical replay/state reconstruction tooling.

## Initial definition of done

A developer can run multiple different agent loops concurrently; spawn hierarchical/parallel children; safely delegate workspace authority; swap run-scoped infrastructure without workflow changes; install a third-party/generated adapter under immutable digest/conformance policy; recover safely from crashes around external effects; audit exact resolved run bindings; and control the Mac-hosted runtime securely from a remote client.
