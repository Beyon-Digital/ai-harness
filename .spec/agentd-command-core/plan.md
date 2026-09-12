# Plan — agentd-command-core

**Status:** draft
**Date:** 2026-09-11

Module spec. Phase 1 input is the approved umbrella plan at
`.spec/agentd-microkernel-mvp/plan.md`. This module is third in the build order; the persistence
module is complete (7/7 tasks, approved branch). Approvals below are recorded under the user's
standing instruction to proceed through design, tasks, and implementation without per-step
confirmation.

## Problem statement

Persistence is real, but nothing routes state changes through it. Any future caller could
begin its own transaction, skip idempotency, ignore the daemon fence, or commit without outbox
rows. The pack's Command Coordinator specification (`specs/command-coordinator.md`) and task
CMD-001 require one linearization path with replay semantics, and none exists yet.

## Outcome

`CommandCoordinator::execute` is the only way a state-changing command reaches the store: it
validates the envelope, opens a fenced write transaction, resolves the idempotency record,
invokes a typed handler that never sees the store, records the outcome, stages outbox rows, and
commits. Pre-commit faults leave zero rows; post-commit faults make replays return the stored
outcome. Tasks CMD-000, CMD-001A, and CMD-001B are implemented and reviewed.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | The fault-injection seam is extended additively: `domain::faults::FaultInjector` gains `fn inject(&self, point: &str) -> bool` with a default `false`, and `testkit::ArmedFaults` fires and reports `true` once per arm | Without a boolean seam the crash tests cannot simulate a failure at a coordinator boundary; an alternative is a coordinator-local injector trait |
| 2 | The coordinator validates the request digest format and compares stored digests; it does not recompute digests | Recomputing would add `sha2` plus canonical serialization to the coordinator; the foundation spec assigns digest computation to clients |
| 3 | Only successful outcomes are recorded in idempotency; failures roll back entirely | If deterministic failures must be replayed, the coordinator needs an error-recording policy and error outcomes in the record |
| 4 | MVP outcome code vocabulary is `ok` plus the payload bytes; richer codes arrive with the commands | Stored outcomes would need a code registry before the first real command |
| 5 | A present `deadline_unix_ms` in the past rejects the command with `FailedPrecondition` before any mutation | Expired deadlines could be treated as best-effort instead |
| 6 | The coordinator does not instrument spans in this module; store observability was deferred by the persistence ruling | Adding `observability` dependency and span plumbing would extend the task |

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| The coordinator algorithm is fully specified | `agent-os-microkernel-mvp-buildpack/specs/command-coordinator.md:26-40` | CMD-001B implements the ten steps and the replay outcome branch verbatim |
| The command envelope fields are fixed | `specs/command-coordinator.md:8-24`, `contracts/control-api/mvp_control.proto` | `CommandEnvelope` mirrors the wire fields; `command_type` is the fully-qualified payload message name (design D2 of the foundation) |
| Required tests and acceptance are fixed | `tasks/CMD-001.md:28-37` | Fault-before-commit, fault-after-commit replay, idempotency conflict, and fence mismatch tests are the task's checklist |
| Transaction recipe A is the normative sequence | `specs/transaction-recipes.md:7-23` | Fenced begin, idempotency lookup, mutations, stream/outbox allocation, idempotency outcome, commit |
| The store port is ready and typed | `agent-os/crates/kernel-store/src/txn.rs`, `repositories.rs`; persistence reports | The coordinator consumes `KernelStore`, `KernelTxn`, `IdempotencyRepo`, `StreamRepo`; no SQL ever crosses this boundary |
| Fault injection exists but cannot force a failure | `agent-os/crates/domain/src/faults.rs:4-6`, `agent-os/crates/testkit/src/faults.rs:18-58` | CMD-001A adds the additive boolean seam and its once-per-arm semantics test |
| Stream staging API exists | `agent-os/crates/events/src/outbox.rs` (`DraftEvent`, `stage`) | Handlers stage events through the txn; the coordinator does not build outbox rows itself |
| Error taxonomy and IDs are ready | `agent-os/crates/errors`, `agent-os/crates/domain` | Errors map to stable codes; envelope ids are typed |

## Existing conventions to follow

- All commands run from `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Pack validators from the repo root: `validate_buildpack.py`, `tools/validate_repo.py`.
- No `unwrap()`/`expect()` outside tests; no payload or secret material in errors or logs;
  no sleeps in tests; parameterized SQL only, and no SQL outside the SQLite crate.
- Commit per task with the task id; reports under this spec's `reports/` directory.

## Approach

### Chosen

Mirror CMD-001 as three small tasks behind one interface:

1. **CMD-000** — declare the `command-coordinator` crate's remaining dependencies
   (`async-trait`, dev `tokio`, `testkit`, `tempfile`) and resolve the lock.
2. **CMD-001A** — extend the fault seam additively: `FaultInjector::inject` defaulting to
   `false`, `ArmedFaults::inject` firing once per arm, with tests proving exactly-once and the
   default no-fault behaviour. This is the smallest change that lets a coordinator boundary
   deterministically fail, without weakening the existing `trigger` contract.
3. **CMD-001B** — implement `envelope.rs`, `handler.rs`, and `lib.rs`: envelope validation,
   the type-erased handler registry over `prost` payload bytes, the ten-step execute algorithm
   with the replay branch, and the fault points `command.before_commit` and
   `command.after_commit`. Integration tests use a real SQLite store in a temp root.

Dependency edges: CMD-000 → CMD-001A → CMD-001B. Sequential; the module is small.

### Rejected

| Alternative | Why not |
|---|---|
| A generic `execute<C: Command>` per command type | The registry must dispatch on a wire string at runtime; generics stop at the API boundary anyway |
| A closed enum of commands in the coordinator | Higher-level commands arrive in later modules and would force edits to the coordinator every time |
| Recording failed outcomes in idempotency | Failure replay policy is unspecified in the pack; success-only recording is safe and simpler (assumption 3) |
| A coordinator-local fault trait instead of extending the foundation one | Duplicates the injector contract and splits the testkit seam; the additive default method is non-breaking |
| Recomputing the request digest inside the coordinator | Duplicates the client contract and drags canonical serialization into this module (assumption 2) |
| Instrumenting spans now | Persistence recorded the observability deferral; adding it here would expand scope without a consumer |

## Scope

**In scope**

- CMD-000, CMD-001A, CMD-001B as described
- The four pack-required tests plus unknown-command, deadline, and registry tests
- The `command.before_commit` and `command.after_commit` fault points
- One representative test handler performing a session insert plus an outbox stage within the
  transaction (the pack's "prove commit/replay semantics before higher-level commands")

**Explicitly out of scope**

- The 18 real command handlers (later modules: runtime-graph, security, config, scheduler)
- Control API transport, authentication, and peer-credential handling (control-api module)
- Request digest computation and canonical serialization (foundation spec)
- Query paths; only mutations flow through the coordinator

## Capability map

Single capability — command routing and linearization. No decomposition; three sequential tasks.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| The additive fault method changes a foundation trait | Low | Domain and testkit consumers | Default implementation keeps every existing implementor compiling; a test pins once-per-arm semantics |
| Registry type erasure around `prost` payloads becomes awkward | Medium | CMD-001B churn | Handlers receive raw payload bytes and decode their own typed message; registration is a plain map insert |
| Crash tests accidentally assert the wrong fault point | Medium | False confidence in replay semantics | Tests assert row counts, outbox rows, and idempotency rows before and after replay, not just error codes |
| Scope creep toward real commands | Low | Coordinator churn | The task forbids command semantics; the test handler lives in tests |

## Parallelisation forecast

Fully sequential: CMD-000 → CMD-001A → CMD-001B. The module is too small to parallelize safely,
and each step is a prerequisite for the next.

## Open questions for the user

None blocking. The five behavioural decisions above (fault seam, digest validation, success-only
recording, outcome vocabulary, deadline rejection) are resolved in the requirements with
proposed defaults and can be corrected at review.

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
