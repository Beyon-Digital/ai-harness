# Tasks — agentd-runtime-graph

**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)
**Design:** `design.md` (approved)

## Global constraints

- All Rust lives under `agent-os/`; no build-pack edits.
- From `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- From the repo root: both pack validators and the contract mirror check.
- No `unwrap()`/`expect()` outside tests; no payload bytes in errors; no sleeps in tests;
  handlers receive context and transaction only.
- Commit per task with the task id; retry `git commit` on `index.lock` after two seconds.

## Execution contract for subagents

1. Claim before writing: `python3 "$SPECFLOW" claim agentd-runtime-graph TASK_ID AGENT_ID` then `start`.
2. Stay inside the task's `files:` lease; `block` and report if another file is needed.
3. `depends_on` interfaces are contracts from `design.md`; do not redesign them.
4. Test first where the task says so; focused test, then the affected suite once.
5. Report `DONE` · `DONE_WITH_CONCERNS` · `BLOCKED` · `NEEDS_CONTEXT` honestly.
6. Never dispatch your own reviewer.
7. Commit scoped to your files, then `review`, then write
   `.spec/agentd-runtime-graph/reports/task-TASK_ID.md`.

---

### Task RUN-000: Runtime entities and creation commands

- status: pending
- owner: -
- depends_on: none
- files: `agent-os/crates/runtime/Cargo.toml`, `agent-os/crates/run-graph/Cargo.toml`, `agent-os/crates/agentd/Cargo.toml`, `agent-os/Cargo.lock`, `agent-os/crates/kernel-store-sqlite/src/repos/agent_specs.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/mod.rs`, `agent-os/crates/runtime/src/lib.rs`, `agent-os/crates/runtime/src/agent_spec.rs`, `agent-os/crates/runtime/src/session.rs`, `agent-os/crates/runtime/src/task.rs`, `agent-os/crates/runtime/src/create_run.rs`, `agent-os/crates/runtime/tests/entities.rs`
- requirements: R1.1, R1.2, R1.3, R1.4, R1.5, R1.6, R1.7, N1, N3, G1, G2
- scope: large
- model: capable

**Objective:** Sessions, immutable agent specs, tasks with graph heads, and `Created` runs are created atomically through the coordinator.

**Context the implementer cannot infer:**

- Dependencies: add to `runtime` — `command-coordinator`, `events`, `kernel-store`, `domain`, `errors`, `async-trait`, `prost`; dev `kernel-store-sqlite`, `testkit`, `tempfile`, `tokio` (macros, rt-multi-thread), `proptest`. To `run-graph` — `domain`, `errors`, `kernel-store`, `events`; dev `testkit`, `tokio`. To `agentd` — `runtime`. All exist in workspace dependencies; append only.
- Handler shapes and command type constants are in the design's `runtime :: handlers` block. Decode payloads with the prost message from `domain::generated::contract`; command type strings are the fully-qualified message names (verify against `commands.proto`).
- The coordinator passes `CommandContext` (principal, actor, command id, correlation) and `&mut dyn KernelTxn`; stage events with `events::outbox::stage` and the catalogued types (`SessionCreated`, `AgentSpecRevisionStored`, `TaskCreated`, `RunCreated`, `ChildRunCreated`). Use the envelope builder with the catalog policy for classification.
- `agent_specs` repository: insert-only; rely on the schema trigger for immutability; identical `(id, version, digest)` is idempotent, different bytes conflict. Wire it into `repos/mod.rs` and the `KernelTxn`/`KernelReadTxn` accessors if an accessor is missing — the port traits are in `kernel-store/src/repositories.rs` and `txn.rs`, both outside this lease: if an accessor is missing, block and name the file.
- Task creation inserts the `run_graph_heads` row in the same transaction (D16). Runs start `Created`; do not transition to `Ready`.
- Child creation: when `parent_run_id` is present, load the parent and require `observed_parent_cancellation_epoch` to equal its persisted `cancellation_epoch`; mismatch or absence → `FailedPrecondition`/`Never`.
- Tests: real coordinator + real SQLite store in a temp root, `TestClock`, `DeterministicIds`; cover immutable spec revisions, idempotent session/task creation, child parent linkage, stale/absent epoch rejection, and that a created run is `Created` (never `Ready`) with outbox and idempotency rows present.

**Steps:**

- [ ] Manifests and lock
- [ ] Write `tests/entities.rs` first (RED), implement the repo and handlers, GREEN
- [ ] Run the full workspace gates
- [ ] Commit: `feat(runtime): entities and creation commands [RUN-000]`

**Acceptance criteria:**

- [ ] R1.1-R1.7 — atomic creation, immutability, epoch check, `Created` only, outbox plus idempotency
- [ ] N1/N3 — no payload bytes in errors; no contract or schema changes
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p runtime -p kernel-store-sqlite` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean

---

### Task RUN-001: Run state machine and revision rules

- status: pending
- owner: -
- depends_on: RUN-000
- files: `agent-os/crates/runtime/src/state.rs`, `agent-os/crates/runtime/src/run.rs`, `agent-os/crates/runtime/tests/state_machine.rs`
- requirements: R2.1, R2.2, R2.3, R2.4, R2.5
- scope: medium
- model: standard

**Objective:** The normative transition table is the only authority for run state, with revisioned mutations and terminal reasons.

**Context the implementer cannot infer:**

- The table is `specs/runtime-manager.md`: `Created -> Ready`; `Ready -> Running | Cancelled`; `Running -> WaitingTool | WaitingChild | WaitingHuman | Suspended | Cancelling | Completed | Failed`; `WaitingTool|WaitingChild|WaitingHuman|Suspended -> Running | Cancelling | Failed`; `Cancelling -> Cancelled | Failed`.
- `transition` loads the run, checks the table, checks `expect_revision`, applies the CAS update with `bump_revision`, stores `terminal_reason` on terminal targets, and returns the updated row. Illegal transitions and revision mismatches are `Conflict`/`Never`.
- Recovery disposition changes do not use this function; they patch `recovery` with `bump_revision` and leave state untouched.
- Tests: table-driven allowed/forbidden cases for every state pair; a proptest over arbitrary attempts asserting legality and non-decreasing revisions; terminal reason persistence.

**Steps:**

- [ ] Write the table and property tests first (RED), implement, GREEN
- [ ] Run `cargo test -p runtime` and gates
- [ ] Commit: `feat(runtime): run state machine [RUN-001]`

**Acceptance criteria:**

- [ ] R2.1-R2.5 — table enforced, revision increment exactly once, terminal reasons, recovery independent
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p runtime` passes
- [ ] clippy and fmt clean

---

### Task RUN-002: Dependency edges and cycle prevention

- status: pending
- owner: -
- depends_on: RUN-001
- files: `agent-os/crates/run-graph/src/lib.rs`, `agent-os/crates/run-graph/src/graph.rs`, `agent-os/crates/run-graph/src/repository.rs`, `agent-os/crates/run-graph/tests/graph.rs`
- requirements: R3.1, R3.2, R3.3, R3.4, R3.5, R3.6, P1, N2
- scope: large
- model: capable

**Objective:** Dependency edges are transactional, same-task scoped, mutable-target only, duplicate-safe, and can never form a committed cycle.

**Context the implementer cannot infer:**

- The persistence `GraphRepo` already provides `ensure_head`, `cas_head_revision`, `insert_dependency` (reachability plus head advance inside the immediate transaction), `list_dependencies`, and `is_reachable`; reuse them rather than reimplementing cycle logic. The service adds: loading source/target and validating same task, mutable target (`Created` or `Ready` with no live claim), staging `DependencyAdded` via `events::outbox::stage`, and returning deterministic outcomes for duplicates.
- Ancestry helpers (`descendants`) walk `runs.parent_run_id` via `RunRepo::list_by_task`; never add a second ancestry table.
- Tests: cycle rejection, opposite-edge barrier race (at most one commits), duplicate edge, state restrictions, cross-task rejection, and a property test that sequential inserts with deliberate pairs keep the graph acyclic.

**Steps:**

- [ ] Write `tests/graph.rs` first (RED), implement `graph.rs`/`repository.rs`, declare modules in `lib.rs`, GREEN
- [ ] Run `cargo test -p run-graph` and gates
- [ ] Commit: `feat(run-graph): dependency edges and cycle prevention [RUN-002]`

**Acceptance criteria:**

- [ ] R3.1-R3.6 — same task, mutable target, cycle rejection, head revision plus event, no second ancestry
- [ ] P1 — acyclic under interleavings
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p run-graph` passes; clippy and fmt clean

---

### Task RUN-003: Readiness and atomic claims

- status: pending
- owner: -
- depends_on: RUN-002
- files: `agent-os/crates/run-graph/src/readiness.rs`, `agent-os/crates/run-graph/src/lib.rs`, `agent-os/crates/runtime/src/claim.rs`, `agent-os/crates/runtime/src/lib.rs`, `agent-os/crates/run-graph/tests/readiness.rs`, `agent-os/crates/runtime/tests/claim.rs`
- requirements: R4.1, R4.2, R4.3, R4.4, R4.5, R4.6, P2, N2
- scope: large
- model: capable

**Objective:** Readiness evaluates the three conditions, and exactly one racer claims a ready run.

**Context the implementer cannot infer:**

- `condition_met` implements `completed_successfully` (COMPLETED), `any_terminal` (any terminal state), `completed_or_cancelled` (COMPLETED or CANCELLED). `dependencies_satisfied` loads the target's dependencies (`GraphRepo::list_dependencies`) and each source run.
- `claim` requires state `Ready`, recovery `Normal`, no live unexpired claim, and satisfied dependencies; it CAS-updates `Ready -> Running` with owner, token (fresh UUIDv7 via the id provider), expiry, and daemon epoch, bumping the revision and staging `RunClaimed` plus `RunStarted`. Expired claims are reclaimable only when recovery is `Normal`.
- `ClaimReadyRunHandler` decodes its payload and calls `claim`; register it in `register_handlers` alongside the RUN-000 handlers.
- Tests: all condition combinations; claim blocked by unmet dependencies, non-Normal recovery, and a live claim; expired-claim reclaim; 100 barrier-synchronized claimants with exactly one `Ok` and one persisted claim. Seed `Ready` runs through repository scaffolding inside a test transaction (documented as test-only).

**Steps:**

- [ ] Write readiness and claim tests first (RED), implement, GREEN
- [ ] Run `cargo test -p run-graph -p runtime` and gates
- [ ] Commit: `feat(runtime): readiness and atomic claims [RUN-003]`

**Acceptance criteria:**

- [ ] R4.1-R4.6 — conditions correct, one winner, blocks honored, expiry policy
- [ ] P2 — exactly one claim under the 100-way race
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p run-graph -p runtime` passes; clippy and fmt clean

---

### Task RUN-004: Cancellation epochs and subtree cancellation

- status: pending
- owner: -
- depends_on: RUN-003
- files: `agent-os/crates/run-graph/src/cancellation.rs`, `agent-os/crates/run-graph/src/lib.rs`, `agent-os/crates/runtime/src/cancel.rs`, `agent-os/crates/runtime/src/lib.rs`, `agent-os/crates/run-graph/tests/cancellation.rs`, `agent-os/crates/runtime/tests/cancel.rs`
- requirements: R5.1, R5.2, R5.3, R5.4, R5.5, P3, N2
- scope: large
- model: capable

**Objective:** Cancellation advances the epoch and propagates through the known subtree; a racing child with a stale observed epoch can never commit.

**Context the implementer cannot infer:**

- `cancel_subtree`: CAS-increment the root's `cancellation_epoch` (bump revision), stage `CancellationEpochAdvanced`, walk descendants via the run-graph ancestry helper, and transition eligible runs: `Created`/`Ready` to `Cancelled`, started non-terminal states to `Cancelling`; skip terminal and already-`Cancelling` runs. Use `runtime::run::transition` for each change so the table and revisions stay authoritative; stage the run-state events.
- `CancelRunHandler` decodes payload and calls the service; register it.
- The RUN-000 child-epoch check is the fence; this task tests the race: with a barrier, hold a `CreateTaskRun` at its epoch check, commit `CancelRun`, then release; the child must be rejected. A nested propagation test and a terminal-child-unchanged test complete the suite.

**Steps:**

- [ ] Write tests first (RED), implement, GREEN
- [ ] Run `cargo test -p run-graph -p runtime` and gates
- [ ] Commit: `feat(runtime): cancellation epochs and subtree propagation [RUN-004]`

**Acceptance criteria:**

- [ ] R5.1-R5.5 and P3 — epoch fence, propagation, terminal unaffected, no escaped child
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p run-graph -p runtime` passes; clippy and fmt clean

---

### Task RUN-005: Startup reconstruction and recovery dispositions

- status: pending
- owner: -
- depends_on: RUN-004
- files: `agent-os/crates/kernel-store/src/repositories.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/runs.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/effects.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/timers.rs`, `agent-os/crates/testkit/src/store.rs`, `agent-os/crates/runtime/src/recovery.rs`, `agent-os/crates/runtime/src/lib.rs`, `agent-os/crates/agentd/src/recovery.rs`, `agent-os/crates/runtime/tests/recovery.rs`
- requirements: R6.1, R6.2, R6.3, R6.4, R6.5, R6.6, P4, N2
- scope: large
- model: capable

**Objective:** Restart classifies every non-terminal run per the recovery matrix and persists dispositions without ever leaving an ambiguous effect resumable.

**Context the implementer cannot infer:**

- Add the three list methods exactly as the design's port-additions block specifies; implement them in the SQLite repos (parameterized) and mirror them in the testkit mock so mock parity holds.
- `reconstruct`: read transaction over `list_active`; for each run load effects (`list_by_run`), timers, and the frozen environment bindings (`EnvironmentRead`); classify with precedence: unknown effect (`BlockedUnknownEffect`), dispatched/ambiguous effect (`NeedsReconciliation`), missing adapter binding (`BlockedMissingResource`), otherwise `Recovering` (or `Normal` for runs with no in-flight work per the matrix). Unmapped combinations fail closed with `Internal` and abort startup.
- Persist dispositions via a write transaction patching `recovery` and bumping the revision; never change state. Stage `RunRecoveryDispositionChanged`.
- `agentd/src/recovery.rs` is a thin call-through; wiring into `main` happens in INT-001.
- Tests: restart with a Running run and no effects; with an unknown effect; with a missing adapter binding; disposition persistence; unmapped-combination fail-closed; terminal runs excluded.

**Steps:**

- [ ] Port list methods plus mock, then `tests/recovery.rs` (RED), implement, GREEN
- [ ] Run the full workspace gates and validators
- [ ] Commit: `feat(runtime): startup reconstruction and recovery dispositions [RUN-005]`

**Acceptance criteria:**

- [ ] R6.1-R6.6 and P4 — enumeration, matrix classification, fail-closed, ambiguous never resumable
- [ ] Mock mirrors the new list methods
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test --workspace` passes; clippy and fmt clean
- [ ] From the repo root: both validators and the mirror check pass

---

## Checkpoints

| After wave | Check | Command |
|---|---|---|
| 1 | Entities and creation green | From `agent-os/`: `cargo test -p runtime -p kernel-store-sqlite` |
| 2 | State machine green | From `agent-os/`: `cargo test -p runtime` |
| 3 | Graph green | From `agent-os/`: `cargo test -p run-graph` |
| 4 | Claims green, one-winner race stable | From `agent-os/`: `cargo test -p runtime -p run-graph` |
| 5 | Cancellation green | Same |
| 6 | Public gates | From `agent-os/`: fmt, clippy, full suite; repo root: both validators plus mirror check |

## Rulings

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | `Ready` is produced only by `BindRun` (config module); claim tests seed it via repository scaffolding | Preserves the environment-freeze invariant | A future refactor of the scaffolding must not leak into production |
| 2 | Cancellation sends never-started runs straight to `Cancelled` | No cleanup to drain | If later policy wants `Cancelling` for all, the table and events already support it |
| 3 | Port gains exactly three list methods | Smallest surface recovery needs | More query methods may be needed by later modules; additive |
