# Task RUN-005 — Startup reconstruction and recovery dispositions

- **Status:** review
- **Agent:** agent-run005
- **Commit:** `2228e86` — `feat(runtime): startup reconstruction and recovery dispositions [RUN-005]`
- **Depends on:** RUN-004 (done)
- **Files written:**
  - `agent-os/crates/kernel-store/src/repositories.rs` — `RunRead::list_active`,
    `TimerRead::list_by_run`
  - `agent-os/crates/kernel-store-sqlite/src/repos/runs.rs` — `list_active`
  - `agent-os/crates/kernel-store-sqlite/src/repos/timers.rs` — `list_by_run`
  - `agent-os/crates/kernel-store-sqlite/src/repos/effects.rs` — **unchanged**;
    `EffectRead::list_by_run` already existed (see Concerns 1)
  - `agent-os/crates/testkit/src/store.rs` — mock list implementations
  - `agent-os/crates/runtime/src/recovery.rs` (new)
  - `agent-os/crates/runtime/src/lib.rs` — module declaration
  - `agent-os/crates/agentd/src/recovery.rs` — thin call-through
  - `agent-os/crates/runtime/tests/recovery.rs` (new)

## Acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| R6.1 — enumerate non-terminal runs and load frozen environments, effects, timers | pass | `list_active` returns `state NOT IN (9,10,11)` ordered by `created_at_ms, run_id` (SQLite) and mirrors that order in the mock; `terminal_runs_are_never_enumerated` asserts exactly one run examined out of four; `classification_follows_the_recovery_matrix` asserts `examined == 23` |
| R6.2 — exactly one disposition per the normative matrix, without changing run state | pass | 23-case table test over real SQLite (all Matrix A rows reachable, every Matrix B in-flight state, safe/unsafe `DISPATCHED` split, `SUSPENDED` rows); `waiting_child_depends_on_its_declared_conditions` (all conditions satisfied → `Recovering`, live child → `Normal`); `stale_claimed_timers_hold_the_run_out_of_normal`; every persistence assertion keeps `state` unchanged |
| R6.3 — unmapped combinations fail closed, never a default | pass | `unmapped_combination_fails_closed_without_mutation`: `Created` + `Prepared` → `Internal`/`Never`, message carries run id + effect id + wire states, no payload bytes, recovery stays `Normal`, revision `0`, no outbox events |
| R6.4 / P4 — ambiguous effect persisted as blocked/reconciliation and never resumable | pass | `unknown_effect_blocks_the_run_and_never_leaves_it_resumable`: persisted `BlockedUnknownEffect`; after scaffolding the run to `Ready`, `claim` rejects `FailedPrecondition`/`Never`; unsafe `DISPATCHED` → `BlockedUnknownEffect`, safe `DISPATCHED` → `NeedsReconciliation` in the table test |
| R6.5 — dispositions recorded durably with audit identifiers | pass | One write transaction per changed run: `RunPatch { recovery, bump_revision: true }` plus a catalogued `RunRecoveryDispositionChanged` (internal/audit) on `run/<run-id>` carrying the full `AgentRun` snapshot at the new revision (`running_run_without_effects_recovers_and_persists_the_disposition`); `repeated_restarts_report_the_same_disposition_without_rewriting` proves change-only persistence (revision stays `1`, single event) |
| R6.6 — non-`Normal` runs get no loop turns | pass (boundary) | Recovery only writes `recovery` and never issues turns; `claim` still gates on `recovery == Normal` (`runtime/src/claim.rs:46`), proven end-to-end in the P4 test. Worker scheduling/wiring is INT-001 |
| Mock mirrors the new list methods | pass | `MockRunRepo::list_active` (terminal filter + `(created_at_ms, run_id)` order) and `MockTimerRepo::list_by_run` mirror the SQLite queries; `testkit` `store_mock` suite (16) green |
| No `unwrap`/`expect`/sleeps outside tests | pass | grep over `runtime/src/recovery.rs` and `agentd/src/recovery.rs` returns nothing |
| No file outside `files:` changed | pass | Commit `2228e86` touches only leased paths; `effects.rs` intentionally unchanged |

## Commands and output

### RED (`cargo test -p runtime --test recovery`, from `agent-os/`)

`runtime::recovery` was stubbed out to capture the tests-before-implementation signal:

```
error[E0432]: unresolved import `runtime::recovery::reconstruct`
  --> crates/runtime/tests/recovery.rs:26:5
   |
26 | use runtime::recovery::reconstruct;
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `reconstruct` in `recovery`

error[E0425]: cannot find type `RecoveryReport` in module `runtime::recovery`
   --> crates/runtime/tests/recovery.rs:361:33

error[E0382]: borrow of moved value: `ids`
   --> crates/runtime/tests/recovery.rs:67:41
```

(The `E0382` was a harness move-order bug; like RUN-002/RUN-004 it was fixed before
GREEN. The unresolved-import errors are the RED signal.)

### GREEN (`cargo test -p runtime --test recovery`)

```
running 10 tests
test prepared_effect_without_a_matching_binding_is_blocked ... ok
test prepared_effect_without_a_frozen_environment_is_blocked ... ok
test repeated_restarts_report_the_same_disposition_without_rewriting ... ok
test running_run_without_effects_recovers_and_persists_the_disposition ... ok
test stale_claimed_timers_hold_the_run_out_of_normal ... ok
test terminal_runs_are_never_enumerated ... ok
test unmapped_combination_fails_closed_without_mutation ... ok
test unknown_effect_blocks_the_run_and_never_leaves_it_resumable ... ok
test waiting_child_depends_on_its_declared_conditions ... ok
test classification_follows_the_recovery_matrix ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.66s
```

### `cargo test -p runtime -p kernel-store-sqlite -p testkit` (from `agent-os/`)

```
tests/recovery.rs:    test result: ok. 10 passed; 0 failed
tests/cancel.rs:      test result: ok. 7 passed; 0 failed
tests/claim.rs:       test result: ok. 13 passed; 0 failed
tests/entities.rs:    test result: ok. 12 passed; 0 failed
tests/state_machine.rs: test result: ok. 12 passed; 0 failed
kernel-store-sqlite (cas, contention, idempotency, immutability, outbox,
outbox_concurrent, props, repos_remaining, rollback): all ok
testkit (daemon_host, prop_faults, store_mock, unit): all ok
```

### `cargo test --workspace` (from `agent-os/`)

```
94 "test result: ok" targets; 0 "test result: FAILED"; no panicked/error lines
```

### `cargo clippy --workspace --all-targets -- -D warnings`

```
    Checking agentd v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.94s
```

### `cargo fmt --check`

```
FMT CLEAN
```

### Repo root validators and mirror check

```
$ python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py
BUILD PACK OK: 59 tasks, 111 markdown files, contracts locked
$ python3 tools/validate_repo.py
OK: 284 markdown, 13 canonical ports, no link/schema/catalog errors
$ bash agent-os/scripts/check-contract-mirror.sh
contract mirror check OK
```

## Implementation notes

- **Port additions.** `RunRead::list_active` filters `Completed|Failed|Cancelled` and orders
  by `created_at_ms, run_id`; the terminal wire values are bound as query parameters
  rather than interpolated. `TimerRead::list_by_run` binds the run id and orders by
  `due_at_ms, timer_id`. `EffectRead::list_by_run` already existed from RUN-000, so no
  change was needed; the mock gained `list_active` and `TimerRead::list_by_run` only.
- **Classification.** A private `EffectClass` reduces the run's effects to the
  highest-precedence class with order `Unknown > DispatchedUnsafe > DispatchedSafe >
  Claimed > Acknowledged > Prepared > None` (settled effects are `None`). `Dispatched`
  is unsafe only when reconciliation is `IMPOSSIBLE`/`UNKNOWN_RECONCILIATION` **and**
  idempotency is `NOT_IDEMPOTENT`/`UNKNOWN_IDEMPOTENCY`, which the matrix blocks. The
  `matrix(run_state, class)` function is the single source of dispositions and returns
  `None` for every pair no row assigns. Matrix C is unreachable because terminal runs
  are never enumerated, so a terminal state reaching classification is treated as
  unmapped (fail closed) rather than guessed.
- **Precedence application.** Fail-closed coverage is checked first; class B/A
  dispositions win next; Matrix D (missing frozen adapter) then overrides only ordinary
  classes; the `WaitingChild` condition check refines the placeholder row; a stale
  claimed timer (`state == Claimed` with `claim_daemon_epoch != current epoch`) holds an
  otherwise-`Normal` run at `Recovering`.
- **Two passes.** The read pass classifies every active run (and can return `Internal`
  before any write); the write pass patches only runs whose computed disposition differs
  from the persisted one, so unchanged dispositions do not bump revisions or duplicate
  audit events. The CAS fences on the observed revision **and** state, so a concurrent
  writer aborts recovery with `Conflict` and rolls the transaction back.
- **Fence and context.** `reconstruct` requires an acquired daemon fence
  (`FailedPrecondition` otherwise) and uses its epoch both for the timer-staleness rule
  and for the write `TxContext`; principal/command ids are minted from
  `SystemIdProvider` because recovery is not a command.
- **agentd.** `run_startup_recovery` is a thin re-exported call-through
  (`runtime::recovery::{Clock, RecoveryReport, reconstruct}`) so agentd needs no direct
  `domain` dependency; the module-level `#![allow(dead_code)]` matches the unwired
  `lock`/worker modules until INT-001 wires it.

## Concerns

1. **`EffectRead::list_by_run` pre-existed.** The design's port-additions block lists it
   under RUN-005, but RUN-000 added the trait method and both implementations. No change
   was made; only `RunRead::list_active` and `TimerRead::list_by_run` are new.
2. **`WaitingHuman` approval expiry.** The matrix's expired-approval row
   (`REQUIRES_HUMAN_DECISION`) is not implemented: the read surface has no
   list-approvals-by-run query and the frozen `approval_request_ids` are opaque bytes.
   `WaitingHuman` classifies `Normal` (the pending row). The approvals module can add the
   check when it owns the query; the table arm is ready.
3. **Fence requirement.** `reconstruct` fails with `FailedPrecondition`/`Never` when no
   daemon fence is held. INT-001 must acquire the fence (lifecycle step 4) before calling
   the hook; the agentd call-through does not acquire one itself.
4. **Inert `clock`.** The disposition patch surface has no `updated_at_ms` column and the
   outbox stamps `occurred_at_ms` from `SystemClock`, so the clock parameter is carried
   for interface stability only (precedent: `run::transition`'s `_now_ms`).
5. **Matrix D breadth.** Missing-resource detection covers the frozen environment row and
   the per-effect adapter id/version/digest binding. The broader Matrix D row-1 checks
   (workspace URI, config generation, environment field validity) have no read surface in
   this module and are not exercised.
6. **Stale-timer rule.** A `Claimed` timer is stale iff its `claim_daemon_epoch` differs
   from the current fence epoch. A timer claimed under the current epoch (impossible
   before workers start, but representable) is treated as live and does not elevate the
   run.
7. **Change-only persistence.** A disposition equal to the persisted value is not
   rewritten. This matches the `RunRecoveryDispositionChanged` event name and avoids
   revision churn on every restart; the report still lists the classification for every
   examined run.

---

## Fix report — review round 1 (commit `aae067b`)

Status: **in review** (not done). Finding addressed: the approved-with-qualification
review found that `WaitingHuman`/expired-approval classifies `Normal` while the matrix
requires `RequiresHumanDecision`, and that the module doc and table test overclaimed full
matrix coverage.

### Changes (documentation only, no behaviour change)

- `runtime/src/recovery.rs` module doc now states that the matrix is implemented for the
  rows the current read surface reaches (effects, timers, adapter bindings) and that the
  `WaitingHuman` expired-approval row is deferred to the approvals module because the port
  has no approvals-by-run read, so a `WaitingHuman` run currently always classifies
  `Normal`.
- `matrix` help text distinguishes the implemented `Suspended` `RequiresHumanDecision`
  row from the deferred expired-approval row, and the `WaitingHuman` arm carries an
  inline comment naming the deferral.
- `runtime/tests/recovery.rs`: `classification_follows_the_recovery_matrix` renamed to
  `classification_follows_the_matrix_rows_the_read_surface_reaches`; the module doc and
  the `WaitingHuman` case now label the `Normal` assertion explicitly as the deferred
  pending-approval behaviour rather than evidence of full coverage. The assertion itself is
  unchanged.

### Commands and output

```
$ cargo test -p runtime
test result: ok. 4 passed   (unit)
test result: ok. 7 passed   (tests/cancel.rs)
test result: ok. 13 passed  (tests/claim.rs)
test result: ok. 12 passed  (tests/entities.rs)
test result: ok. 10 passed  (tests/recovery.rs)
test result: ok. 12 passed  (tests/state_machine.rs)

$ cargo clippy -p runtime --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 06s

$ cargo fmt --check
FMT CLEAN
```

### Open concerns (fix round)

1. The deferral itself stands (Concern 2 of the main report): the approvals module must
   add an approvals-by-run/expiry read and the `RequiresHumanDecision` arm for
   `WaitingHuman`; the table row is the only unimplemented matrix row the current surface
   cannot reach.
2. All other open concerns from the main report are unchanged.
