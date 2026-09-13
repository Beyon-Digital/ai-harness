# Task RUN-003 — Readiness and atomic claims

- **Status:** review
- **Agent:** agent-run003
- **Commit:** `a42144a` — `feat(runtime): readiness and atomic claims [RUN-003]`
- **Depends on:** RUN-002 (done)
- **Files written:**
  - `agent-os/crates/run-graph/src/readiness.rs`
  - `agent-os/crates/run-graph/src/lib.rs`
  - `agent-os/crates/run-graph/tests/readiness.rs`
  - `agent-os/crates/runtime/src/claim.rs`
  - `agent-os/crates/runtime/src/lib.rs`
  - `agent-os/crates/runtime/tests/claim.rs`
  - `agent-os/crates/runtime/tests/state_machine.rs`

## Acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| R4.1 — exactly the three conditions | pass | `condition_met_matrix_is_exact_for_every_state` (all 12 states × 3 conditions + `Unspecified` false); `dependency_conditions_are_honored_over_persisted_states` (13 persisted-state cases over real edges) |
| R4.2 — claim verifies dependencies, recovery `Normal`, live claim, state | pass | `claim_rejects_unmet_dependencies_without_mutation` (`FailedPrecondition`/`Never`, no mutation/events); `claim_accepts_satisfied_dependencies` (Completed, Failed/any_terminal, Cancelled/completed_or_cancelled); `claim_rejects_non_normal_recovery_without_mutation` (5 persisted non-Normal dispositions); `claim_rejects_a_live_claim_without_mutation` (`Conflict`/`Never`, holder's columns intact) |
| R4.3 — CAS `Ready -> Running` with owner/token/expiry/epoch, revision once | pass | `claim_persists_fence_columns_and_stages_claimed_and_started` (state, revision 1, owner, non-zero token, `now + ttl`, daemon epoch, `RunClaimed` then `RunStarted` on `run/<id>` sequences 1–2, `AgentRun` payload decodes at state RUNNING/revision 1); `claim_handler_claims_a_ready_run_through_the_coordinator` (daemon epoch from `txn.context()`, idempotent replay) |
| R4.4 / P2 — exactly one winner of a 100-way race | pass | `hundred_claimants_yield_exactly_one_winner` (barrier-synchronized 100 writers, exactly 1 `Ok`, 99 `Conflict`/`Never`, one persisted claim, revision 1, exactly two events) |
| R4.5 — expired claims reclaimable only under `Normal` | pass | `expired_claims_are_reclaimed_only_under_normal` (expiry `== now` reclaims with a fresh owner/token/expiry; the same expired claim under `NeedsReconciliation` is `FailedPrecondition` and untouched) |
| R4.6 — blocked runs rejected without mutation | pass | `claim_rejects_every_non_ready_state_without_mutation` (10 non-`Ready` states `Conflict`/`Never`); recovery matrix above; every rejection test re-reads the row and the outbox |
| Handler + registration | pass | `claim_handler_rejects_malformed_payloads_without_mutation` (`InvalidArgument`/`Never`, no bytes echoed, no idempotency record); `claim_handler_requires_a_run_and_an_owner`; `register_handlers_registers_claim_ready_run` (4 registered commands incl. `agentos.spec.v1.ClaimReadyRun`) |
| RUN-001 follow-up — modules declared, `#[path]` removed | pass | `runtime/src/lib.rs` declares `pub mod run; pub mod state;`; `state_machine.rs` now `use runtime::{run, state};` with both `#[path]` includes deleted (11/11 still green) |
| No file outside `files:` changed | pass | `git show --stat a42144a` lists exactly the seven leased paths |
| No `unwrap`/`expect` outside tests; no sleeps | pass | `readiness.rs`/`claim.rs` contain none; the race uses a `tokio::sync::Barrier` |

## Commands and output

### RED (`cargo test -p run-graph --test readiness`)

```
error[E0432]: unresolved import `run_graph::readiness`
  --> crates/run-graph/tests/readiness.rs:17:16
   |
17 | use run_graph::readiness::{condition_met, dependencies_satisfied};
   |                ^^^^^^^^^ could not find `readiness` in `run_graph`

error: could not compile `run-graph` (test "readiness") due to 1 previous error
```

### RED (`cargo test -p runtime --test claim`)

```
27 | use runtime::claim::claim;
   |              ^^^^^ could not find `claim` in `runtime`

error[E0432]: unresolved import `runtime::CMD_CLAIM_READY_RUN`
  --> crates/runtime/tests/claim.rs:28:15
   |
28 | use runtime::{CMD_CLAIM_READY_RUN, RuntimeDeps};
   |               ^^^^^^^^^^^^^^^^^^^ no `CMD_CLAIM_READY_RUN` in the root

error: could not compile `runtime` (test "claim") due to 3 previous errors
```

(The third error was the test harness's own move-order bug — `ids` moved before
`PrincipalId::new(ids.as_ref())` — fixed before GREEN, as in RUN-002.)

### GREEN (`cargo test -p run-graph -p runtime`, from `agent-os/`)

```
     Running tests/graph.rs ...
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.91s
     Running tests/readiness.rs ...
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s
     Running unittests src/lib.rs (runtime) ...
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/claim.rs ...
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
     Running tests/entities.rs ...
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
     Running tests/state_machine.rs ...
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.26s
```

### `cargo test --workspace` (from `agent-os/`)

```
workspace exit=0
91 "test result: ok" targets
grep for "test result: FAILED|panicked|error[" returns nothing
```

### `cargo clippy --workspace --all-targets -- -D warnings`

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.83s
exit=0
```

### `cargo fmt --check`

```
FMT CLEAN
```

## Implementation notes

- **`readiness.rs`.** `condition_met` is `const` (`matches!`, not `==`, because
  the mirror enum's `PartialEq` is not const-callable); `Unspecified` is never
  satisfied. `dependencies_satisfied` loads the target (missing → `NotFound`),
  filters `GraphRepo::list_dependencies` to the target, and loads every source;
  a missing source is `NotFound`, never a guessed state.
- **`claim.rs`.** Validation order: load → state `Ready` (`Conflict`) →
  recovery `Normal` (`FailedPrecondition`) → no live unexpired claim
  (`Conflict`, expiry `> now_ms`) → dependencies satisfied
  (`FailedPrecondition`). The CAS fences on the observed revision *and*
  `state = Ready`, writes the claim columns, bumps the revision once, then
  stages `RunClaimed` and `RunStarted` (both `contract::AgentRun` payloads on
  `run/<id>`; no dedicated messages exist in the frozen contract). Two
  intermediate GREEN failures were real and are covered by the final tests:
  an unmasked UUIDv7 token exceeded SQLite's signed-64 range
  (`runs.claim_token`), and `RecoveryDisposition::Unspecified` violates the
  schema `CHECK (recovery_disposition BETWEEN 1 AND 6)` and so cannot be seeded.
- **Handler.** Decodes `contract::ClaimReadyRun`, requires `run_id` and a
  non-empty `claim_owner` (`InvalidArgument` before any read), takes `now_ms`
  from `RuntimeDeps.clock` and `daemon_epoch` from `txn.context()`, and returns
  the run id as the outcome payload. Registered in `register_handlers`.
- **Test scaffolding.** `Ready` is produced only by `BindRun` (config module),
  so `seed_run`/`seed_claim`/`seed_recovery` write through the repository in a
  test transaction; this is documented test-only scaffolding (D4), never a
  production path.
- **Race.** 100 multi-thread tasks wait on one `tokio::sync::Barrier`, each
  opening its own `BEGIN IMMEDIATE` transaction; the store config uses 8 pooled
  connections and a 30 s busy timeout for headroom. No sleeps.

## Concerns

1. **Token source.** The design's `claim` signature carries no `IdProvider`, so
   `fresh_token` mints a `SystemIdProvider` UUIDv7 masked to `i64::MAX` (the
   `claim_token` column's range) — the same precedent RUN-002 used for
   dependency/event ids. Tests assert token presence/freshness, not fixed
   values. Threading `RuntimeDeps.ids` through would need a signature change
   (the ledger already tracks the analogous RUN-002 minor).
2. **Reclamation scope.** Per the task's "requires state `Ready`", an expired
   claim is reclaimable only on a `Ready` run (the synthetic D4 state; a crash
   leaves the run `Running`). Restoring a crashed `Running` run to `Ready` is
   the startup-recovery domain (RUN-005), not this service.
3. **Event correlation.** `claim` carries no `CommandContext`, so the two
   staged events have no `correlation_id`. The design signature is silent;
   adding one is a small interface extension if the reviewer wants it.
4. **`Unspecified` recovery cannot be tested end-to-end** because the schema
   rejects persisting it; the non-Normal matrix covers all five persisted
   non-Normal dispositions.
