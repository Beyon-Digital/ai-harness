# Task RUN-001 — Run state machine and revision rules

- **Status:** review
- **Agent:** agent-run001
- **Commit:** `c36dc27` — `feat(runtime): run state machine [RUN-001]`
- **Depends on:** RUN-000 (done)
- **Files written:**
  - `agent-os/crates/runtime/src/state.rs`
  - `agent-os/crates/runtime/src/run.rs`
  - `agent-os/crates/runtime/tests/state_machine.rs`

## Acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| R2.1 — transition table is the only authority | pass | `normative_table_is_matched_for_every_state_pair` (all 144 pairs), `every_allowed_pair_commits_once_and_persists_the_target` (24 allowed pairs), `every_forbidden_pair_conflicts_without_mutation` (120 forbidden pairs) |
| R2.2 — illegal transitions are `Conflict`/`Never` with no mutation | pass | forbidden-pair test commits the rejected transaction before re-reading, so any smuggled write would be observed |
| R2.3 — `run_revision` increments exactly once per committed mutation; stale writers lose | pass | allowed-pair test (0 → 1), `sequential_transitions_increment_revision_once_per_commit` (0 → 6), `stale_and_future_revisions_conflict_without_mutation`, proptest |
| R2.4 — terminal targets persist a stable `terminal_reason`; terminal states reject further transitions | pass | `terminal_reason_defaults_are_stable_and_caller_reasons_win`, `terminal_states_reject_further_transitions_and_keep_the_reason` |
| R2.5 — recovery disposition changes are independent of run state | pass | `recovery_disposition_changes_leave_state_and_run_state_untouched` (recovery patch bumps revision, state untouched; a later transition preserves the disposition) |
| No file outside `files:` changed | pass | `git show --stat c36dc27` lists exactly the three leased paths |
| Proptest over arbitrary attempts | pass | `arbitrary_attempt_sequences_keep_revisions_monotonic_and_states_legal` (32 cases, arbitrary initial persisted state, 0–15 arbitrary attempts with stale/current revisions and arbitrary reasons) |

## Commands and output

### RED (`cargo test -p runtime --test state_machine`, before implementation)

```
error: couldn't read `crates/runtime/tests/../src/state.rs`: No such file or directory (os error 2)
  --> crates/runtime/tests/state_machine.rs:22:1
   |
22 | mod state;
   | ^^^^^^^^^^

error: could not compile `runtime` (test "state_machine") due to 1 previous error
```

### GREEN (`cargo test -p runtime --test state_machine`)

```
running 11 tests
test missing_runs_are_not_found ... ok
test normative_table_is_matched_for_every_state_pair ... ok
test recovery_disposition_changes_leave_state_and_run_state_untouched ... ok
test sequential_transitions_increment_revision_once_per_commit ... ok
test stale_and_future_revisions_conflict_without_mutation ... ok
test every_allowed_pair_commits_once_and_persists_the_target ... ok
test terminal_states_are_exactly_completed_failed_and_cancelled ... ok
test terminal_reason_defaults_are_stable_and_caller_reasons_win ... ok
test terminal_states_reject_further_transitions_and_keep_the_reason ... ok
test every_forbidden_pair_conflicts_without_mutation ... ok
test arbitrary_attempt_sequences_keep_revisions_monotonic_and_states_legal ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.89s
```

### `cargo test -p runtime` (from `agent-os/`)

```
tests/entities.rs: 12 passed; 0 failed
tests/state_machine.rs: 11 passed; 0 failed
Doc-tests runtime: 0 passed; 0 failed
```

### `cargo test --workspace` (from `agent-os/`)

```
exit=0
88 "test result: ok" targets; 0 failures (grep for "test result: FAILED|panicked|error[" returns nothing)
```

### `cargo clippy --workspace --all-targets -- -D warnings`

```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 34.18s
```

### `cargo fmt --check`

```
FMT CLEAN
```

## Implementation notes

- `state.rs` encodes the normative table from `specs/runtime-manager.md` in
  `allows` (const) and the absorbing states in `is_terminal` (const), plus the
  four reason constants from the design interface.
- `run.rs::transition` loads the run (`NotFound` when absent), checks the
  table, checks `expect_revision`, applies `cas_update` with
  `state = Some(from)` in `RunCas` and `bump_revision = true`, and returns the
  re-read row. Every rejection is `Conflict`/`Never` (or `NotFound`/`Never`)
  with no write.
- Terminal targets store `reason` when the caller supplies one, otherwise the
  canonical `completed`/`failed`/`cancelled` token. Non-terminal targets never
  write `terminal_reason`; recovery changes are a separate CAS patch on
  `recovery` with `bump_revision`.

## Concerns

1. **`runtime/src/lib.rs` is not in this lease**, so `state`/`run` are not yet
   declared on the library target; the test suite compiles them with
   `#[path = "../src/state.rs"]` / `#[path = "../src/run.rs"]`, matching the
   existing `agentd/tests/lock_exclusion.rs` precedent. RUN-003 owns
   `runtime/src/lib.rs` and must add `pub mod state; pub mod run;` before other
   crates use `runtime::run::transition`.
2. **`now_ms` is inert.** `RunPatch`/`RunRepo` expose no `updated_at_ms`
   column, so `transition` accepts the timestamp as `_now_ms` for interface
   stability but cannot persist it. If `updated_at_ms` stamping is required,
   the port needs a patch field (outside this lease).
3. **Reason semantics.** `REASON_CANCELLATION_REQUESTED` is exposed for the
   cancellation service (RUN-004); `transition` itself stores reasons only on
   terminal targets, per the task context ("stores `terminal_reason` on
   terminal targets").
