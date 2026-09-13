# Tasks — agentd-command-core

**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)
**Design:** `design.md` (approved)

## Global constraints

- All Rust lives under `agent-os/`; no build-pack edits in this module.
- From `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- From the repo root: `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`
  and `python3 tools/validate_repo.py`.
- No `unwrap()`/`expect()` outside tests; no payload or secret bytes in errors or logs; no
  sleeps in tests; no SQL outside the SQLite crate.
- Commit per task with the task id; retry `git commit` on `index.lock` after two seconds, up
  to five times.

## Execution contract for subagents

1. Claim before writing: `python3 "$SPECFLOW" claim agentd-command-core TASK_ID AGENT_ID` then `start`.
2. Stay inside the task's `files:` lease; `block` and report if another file is needed.
3. `depends_on` interfaces are contracts from `design.md`; do not redesign them.
4. Test first where the task says so; run the focused test, then the affected suite once.
5. Report `DONE` · `DONE_WITH_CONCERNS` · `BLOCKED` · `NEEDS_CONTEXT` honestly.
6. Never dispatch your own reviewer.
7. Commit scoped to your files, then `review`, then write
   `.spec/agentd-command-core/reports/task-TASK_ID.md`.

---

### Task CMD-000: Declare command-coordinator dependencies

- status: done
- owner: agent-cmd000
- depends_on: none
- files: `agent-os/crates/command-coordinator/Cargo.toml`, `agent-os/Cargo.lock`
- requirements: N3, G1, G2
- scope: small
- model: cheap

**Objective:** The coordinator crate resolves everything its implementation and tests need.

**Context the implementer cannot infer:**

- The crate already declares `domain`, `errors`, `event-journal`, `events`, `kernel-store`.
- Add `async-trait` as a dependency. Add dev-dependencies `tokio` (macros, rt-multi-thread), `testkit`, `tempfile`, and `kernel-store-sqlite` so integration tests can run a real store and testkit doubles.
- All five entries already exist in `[workspace.dependencies]`; use `.workspace = true`. Append only; do not reorder existing lines.
- No production crate may depend on testkit; this is a dev-dependency only.

**Steps:**

- [ ] Edit the manifest; resolve the lock with `cargo check --workspace`
- [ ] Run `cargo test --workspace --no-run`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`
- [ ] Commit: `chore(workspace): declare command-coordinator dependencies [CMD-000]`

**Acceptance criteria:**

- [ ] Dependencies resolve and the lock updates
- [ ] G1 — full workspace check, tests, clippy, and fmt still pass
- [ ] N3 — no contract, schema, or build-pack file changed
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo check --workspace && cargo test --workspace --no-run` exits 0
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

---

### Task CMD-001A: Additive fault-injection seam

- status: done
- owner: agent-cmd001a
- depends_on: CMD-000
- files: `agent-os/crates/domain/src/faults.rs`, `agent-os/crates/testkit/src/faults.rs`, `agent-os/crates/testkit/tests/prop_faults.rs`
- requirements: R4.3, R4.4, N2, P2
- scope: small
- model: standard

**Objective:** A test can deterministically force a failure at a named coordinator boundary without a panic, while existing `trigger` semantics stay untouched.

**Context the implementer cannot infer:**

- Add to `domain::faults::FaultInjector`: `fn inject(&self, point: &str) -> bool { let _ = point; false }`. This is additive and defaulted; every existing implementor keeps compiling.
- `testkit::ArmedFaults::inject` fires an armed point exactly once per arm (marking it triggered, as `trigger` does) and returns `true` when it fired; otherwise returns `false`. Re-arming resets the state.
- `NoFaults` inherits the default and always returns `false`.
- The existing `trigger`/`is_triggered`/`assert_triggered` semantics and tests must not change.
- Test additions in `prop_faults.rs`: an unarmed point returns false; an armed point returns true exactly once; a re-armed point fires again; `inject` marks the point triggered for `assert_triggered`.

**Steps:**

- [ ] Write the failing inject tests first; confirm RED (missing method)
- [ ] Implement the defaulted trait method and the `ArmedFaults` override
- [ ] Run `cargo test -p domain -p testkit` to GREEN; keep existing fault tests passing
- [ ] Run `cargo clippy -p domain -p testkit --all-targets -- -D warnings` and `cargo fmt --check`
- [ ] Commit: `feat(faults): add defaulted inject seam [CMD-001A]`

**Acceptance criteria:**

- [ ] R4.3 — armed points fire exactly once per arm; unarmed points are no-ops
- [ ] R4.4 — `NoFaults` and the default never fire
- [ ] N2 — no sleeps; existing once-per-arm property test still passes
- [ ] P2 — the seam is usable for pre-commit failure injection
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p domain -p testkit` passes
- [ ] `cargo clippy -p domain -p testkit --all-targets -- -D warnings` clean

---

### Task CMD-001B: The command coordinator

- status: done
- owner: agent-cmd001b
- depends_on: CMD-001A
- files: `agent-os/crates/command-coordinator/src/lib.rs`, `agent-os/crates/command-coordinator/src/envelope.rs`, `agent-os/crates/command-coordinator/src/handler.rs`, `agent-os/crates/command-coordinator/tests/coordinator.rs`
- requirements: R1.1, R1.2, R1.3, R1.4, R1.5, R1.6, R2.1, R2.2, R2.3, R2.4, R2.5, R3.1, R3.2, R3.3, R3.4, R4.1, R4.2, N1, P1
- scope: large
- model: capable

**Objective:** The ten-step execute algorithm is real: validation, fenced transaction, idempotent replay, typed handler dispatch, outbox staging, outcome recording, commit, and the two fault points.

**Context the implementer cannot infer:**

- Exact signatures for `RequestDigest`, `CommandEnvelope`, `CommandContext`, `OutcomeCode`, `CommandOutcome`, `CommandHandler`, `CommandRegistry`, `FenceProvider`, `FixedFence`, and `CommandCoordinator` are in `design.md` Interfaces; follow them literally.
- Execute order: registry lookup (unknown → `InvalidArgument`, no transaction); `validate(clock.now_unix_ms())`; `begin_write` with the provider epoch; idempotency lookup (same digest → return stored outcome after dropping the txn; different → `Conflict`); handler with context and `&mut dyn KernelTxn`; `inject(BEFORE_COMMIT)` → on true return `Unavailable`/`Safe` and drop; insert the idempotency record with `OutcomeCode::Ok` and the payload; `commit`; `inject(AFTER_COMMIT)` → on true return `Unavailable`/`Safe`; return the outcome.
- The persistence port names: `KernelStore::begin_write(TxContext)`, `KernelTxn::idempotency()`, `IdempotencyRepo::lookup/insert`, `TxContext`, and the row types in `kernel-store::models`. Insert uses `created_at_ms` from the injected clock.
- `events::outbox::stage(txn, draft)` is the only event staging path for handlers; do not construct outbox rows in this crate.
- Tests in `tests/coordinator.rs` use a real `SqliteKernelStore` in a `tempfile` temp dir, `testkit::TestClock`, `testkit::DeterministicIds`, and `testkit::ArmedFaults`. Define the representative test handler there: it inserts a session (via `txn.sessions().insert`) and stages one outbox event via `events::outbox::stage`, returning `CommandOutcome { code: Ok, payload: b"created" }`.
- Required cases: fresh command commits rows plus outbox plus idempotency; identical replay returns the stored outcome with unchanged counts; different digest → `Conflict`; pre-commit fault → zero rows of any kind and a re-submission succeeds afresh; post-commit fault → error returned, then replay returns the stored outcome with unchanged counts; stale fence → `FailedPrecondition` with no rows; unknown command type → `InvalidArgument` with no transaction; duplicate registration → `Conflict`; malformed digest and past deadline reject. No sleeps.
- `CommandRegistry::register` takes an `Arc⟨dyn CommandHandler⟩`; the duplicate check must be deterministic.

**Steps:**

- [ ] Write the integration test file first with the representative handler and the first three cases; confirm RED
- [ ] Implement `envelope.rs`, `handler.rs`, then the coordinator in `lib.rs`
- [ ] Run `cargo test -p command-coordinator --test coordinator` to GREEN; add the remaining cases
- [ ] Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`
- [ ] Commit: `feat(command): coordinator execute path and replay semantics [CMD-001B]`

**Acceptance criteria:**

- [ ] R1.1–R1.6 — single path, replay, conflict, unknown type, atomic outcome plus outbox, no query routing
- [ ] R2.1–R2.5 — validation, deadline, fenced begin from the provider, zero rows on rejection
- [ ] R3.1–R3.4 — handlers see context and txn only; registry rejects duplicates; test handler uses the same trait object
- [ ] R4.1/R4.2 — pre-commit zero rows; post-commit replay returns the outcome
- [ ] P1 — repeated replays leave row counts unchanged
- [ ] N1 — error messages name fields and points, never payload bytes
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p command-coordinator -p kernel-store-sqlite -p events` passes
- [ ] `cargo test --workspace` passes; `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean

---

## Checkpoints

| After wave | Check | Command |
|---|---|---|
| 1 | Dependencies resolved; lock updated | From `agent-os/`: `cargo check --workspace` |
| 2 | Fault seam green without regressions | From `agent-os/`: `cargo test -p domain -p testkit` |
| 3 | Public gates | From `agent-os/`: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; from the repo root: both validators |

## Rulings

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | The fault seam is extended additively with `inject` rather than a coordinator-local trait | One testkit seam; non-breaking default | A foundation trait edit recorded in this spec's ledger |
| 2 | Success-only idempotency recording and `ok`-only outcome vocabulary | Failure replay is unspecified; safe default | Richer outcome codes need a record change before failures can replay |
| 3 | The coordinator defers span instrumentation, matching the persistence observability ruling | No consumer yet; keeps the dependency surface minimal | Store and command signals stay dark until the owning module instruments them |
