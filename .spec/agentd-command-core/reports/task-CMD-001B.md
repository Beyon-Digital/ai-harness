# Task CMD-001B Report

- **Spec:** agentd-command-core
- **Task:** CMD-001B — The command coordinator
- **Agent:** agent-cmd001b
- **Status:** DONE
- **Commit:** `65494f9` — `feat(command): coordinator execute path and replay semantics [CMD-001B]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What changed

- `agent-os/crates/command-coordinator/src/envelope.rs` (new): `RequestDigest`
  with strict `FromStr` (64 lowercase hex characters, canonical lowercase only)
  and lowercase-hex `Display`, plus `CommandEnvelope` with
  `validate(now_unix_ms)` rejecting empty keys, empty command types
  (`InvalidArgument`/`Never`) and past deadlines (`FailedPrecondition`/`Never`).
- `agent-os/crates/command-coordinator/src/handler.rs` (new): `CommandContext`,
  the `ok`-only `OutcomeCode` (`as_str`, fail-closed `from_stored`), the
  `CommandOutcome` payload pair, the `CommandHandler` async trait over
  `&CommandContext` and `&mut dyn kernel_store::KernelTxn` only, and
  `CommandRegistry` with deterministic once-only `register` (`Conflict` on a
  duplicate, existing handler left in place).
- `agent-os/crates/command-coordinator/src/lib.rs`: `BEFORE_COMMIT` /
  `AFTER_COMMIT` fault points, `FenceProvider` + `FixedFence`, and
  `CommandCoordinator::execute` running the ten-step algorithm in the literal
  design order: registry lookup (unknown → `InvalidArgument`, no transaction),
  `validate(clock.now_unix_ms())`, `begin_write` with the provider epoch,
  in-transaction idempotency lookup (same digest → stored outcome after
  dropping the txn; different → `Conflict`), handler with context and
  `&mut dyn KernelTxn`, `inject(BEFORE_COMMIT)` (`Unavailable`/`Safe` + drop),
  idempotency insert with `created_at_ms` from the injected clock, `commit`,
  `inject(AFTER_COMMIT)` (`Unavailable`/`Safe`, committed state retained), then
  the outcome. Handlers stage events only through `events::outbox::stage`; the
  crate contains no SQL.
- `agent-os/crates/command-coordinator/tests/coordinator.rs` (new): the
  representative handler (session insert through `txn.sessions()`, one outbox
  event through `events::outbox::stage`, `CommandOutcome { Ok, b"created" }`),
  a real `SqliteKernelStore` in a `tempfile` root with `TestClock`,
  `DeterministicIds`, and `ArmedFaults`, and the ten required cases: fresh
  commit, identical replay, digest conflict, pre-commit fault + resubmission,
  post-commit fault + replay, stale fence, unknown type, duplicate
  registration, malformed digest, and past deadline (including the boundary
  where `deadline_unix_ms == now` is accepted).

No SQL, no sleeps, no `unwrap()`/`expect()` outside tests, and no error path
echoes payload bytes.

## Acceptance criteria

| # | Criterion | Met | Evidence |
|---|---|---|---|
| 1 | R1.1–R1.6 — single path, replay, conflict, unknown type, atomic outcome plus outbox, no query routing | **MET** | `fresh_command_commits_rows_outbox_and_idempotency`, `identical_replay_returns_the_stored_outcome_without_changes`, `different_digest_for_the_same_key_is_a_conflict`, `unknown_command_type_rejects_without_rows` pass; `execute` is the only mutating entry point |
| 2 | R2.1–R2.5 — validation, deadline, fenced begin from the provider, zero rows on rejection | **MET** | `past_deadline_is_rejected_without_rows` (zero session/idempotency/outbox rows, `FailedPrecondition`/`Never`), `stale_fence_rejects_without_rows` (real epoch advanced to 2), `TxContext` built from `fence.epoch()` |
| 3 | R3.1–R3.4 — handlers see context and txn only; registry rejects duplicates; test handler uses the same trait object | **MET** | `CommandHandler::handle(&CommandContext, &mut dyn KernelTxn, Vec<u8>)`; `duplicate_registration_is_a_conflict`; the test handler is registered as `Arc<dyn CommandHandler>` and invoked through the trait |
| 4 | R4.1/R4.2 — pre-commit zero rows; post-commit replay returns the outcome | **MET** | `pre_commit_fault_rolls_back_and_resubmission_succeeds`, `post_commit_fault_reports_unavailable_then_replay_returns_the_outcome` |
| 5 | P1 — repeated replays leave row counts unchanged | **MET** | replay test compares full session/idempotency/outbox observations before and after |
| 6 | N1 — error messages name fields and points, never payload bytes | **MET** | messages name `idempotency_key`, `request_digest`, `command_type`, `deadline_unix_ms`, and the fault point constants; no payload is interpolated anywhere |
| 7 | No file outside `files:` changed | **MET** | commit `65494f9` touches exactly the 4 leased paths (`git show --stat 65494f9`) |
| 8 | `cargo test -p command-coordinator -p kernel-store-sqlite -p events` passes | **MET** | exit 0; all suites green |
| 9 | `cargo test --workspace` passes; clippy `-D warnings`; `cargo fmt --check` clean | **MET** | workspace: 82 suites `test result: ok`, 0 failures; clippy exit 0; fmt exit 0 after `cargo fmt -p command-coordinator` |
| 10 | Commit scoped with task id, retry on `index.lock` | **MET** | commit message carries `[CMD-001B]`; retry loop implemented (no retry needed) |

## RED output (real, before implementation)

```
$ cargo test -p command-coordinator --test coordinator
   Compiling command-coordinator v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/command-coordinator)
error[E0432]: unresolved import `command_coordinator::envelope`
  --> crates/command-coordinator/tests/coordinator.rs:11:26
   |
11 | use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
   |                          ^^^^^^^^ could not find `envelope` in `command_coordinator`

error[E0432]: unresolved import `command_coordinator::handler`
  --> crates/command-coordinator/tests/coordinator.rs:12:26
   |
12 | use command_coordinator::handler::{
   |                          ^^^^^^^ could not find `handler` in `command_coordinator`

error[E0432]: unresolved imports `command_coordinator::BEFORE_COMMIT`, `command_coordinator::CommandCoordinator`, `command_coordinator::FixedFence`
  --> crates/command-coordinator/tests/coordinator.rs:15:27
   |
15 | use command_coordinator::{BEFORE_COMMIT, CommandCoordinator, FixedFence};
   |                           ^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^  ^^^^^^^^^^ no `FixedFence` in the root
   |                           |              |
   |                           |              no `CommandCoordinator` in the root
   |                           no `BEFORE_COMMIT` in the root

error: could not compile `command-coordinator` (test "coordinator") due to 3 previous errors
```

## GREEN output — first four cases (after implementing envelope/handler/coordinator)

```
$ cargo test -p command-coordinator --test coordinator
running 4 tests
test different_digest_for_the_same_key_is_a_conflict ... ok
test identical_replay_returns_the_stored_outcome_without_changes ... ok
test fresh_command_commits_rows_outbox_and_idempotency ... ok
test pre_commit_fault_rolls_back_and_resubmission_succeeds ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
```

## GREEN output — full crate after the remaining six cases

```
$ cargo test -p command-coordinator
running 5 tests
test envelope::tests::malformed_digests_are_rejected ... ok
test handler::tests::duplicate_registration_is_a_conflict ... ok
test envelope::tests::canonical_digests_round_trip_through_lowercase_hex ... ok
test handler::tests::empty_command_types_are_rejected ... ok
test handler::tests::outcome_codes_round_trip_and_fail_closed ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 10 tests
test duplicate_registration_is_a_conflict ... ok
test malformed_digest_is_rejected ... ok
test past_deadline_is_rejected_without_rows ... ok
test fresh_command_commits_rows_outbox_and_idempotency ... ok
test identical_replay_returns_the_stored_outcome_without_changes ... ok
test different_digest_for_the_same_key_is_a_conflict ... ok
test stale_fence_rejects_without_rows ... ok
test pre_commit_fault_rolls_back_and_resubmission_succeeds ... ok
test post_commit_fault_reports_unavailable_then_replay_returns_the_outcome ... ok
test unknown_command_type_rejects_without_rows ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.03s

Doc-tests command_coordinator: test result: ok. 0 passed; 0 failed
```

## Gate output

```
$ cargo test -p command-coordinator -p kernel-store-sqlite -p events   # exit 0, all suites ok
$ cargo test --workspace                                               # exit 0, 82 suites "test result: ok", 0 failed
$ cargo clippy --workspace --all-targets -- -D warnings                # exit 0, no diagnostics
$ cargo fmt --check                                                    # exit 0, no output
```

## Files changed

- `agent-os/crates/command-coordinator/src/envelope.rs`
- `agent-os/crates/command-coordinator/src/handler.rs`
- `agent-os/crates/command-coordinator/src/lib.rs`
- `agent-os/crates/command-coordinator/tests/coordinator.rs`

## Concerns

- `OutcomeCode::from_stored` maps unrecognized persisted tokens to
  `Internal`/`Never`, matching the storage layer's fail-closed decoding
  convention; no new error codes were introduced.
- Success-only idempotency means failed commands never replay (design ruling 2);
  a future richer outcome vocabulary needs a record-format decision first.
- The post-commit fault returns `Unavailable`/`Safe` while the work is durably
  committed; callers must replay to observe the stored outcome, as the test
  demonstrates.
- `RequestDigest` parsing rejects uppercase hex, matching the repository's
  canonical-lowercase convention for identifiers and digests.
- The dispatched instruction referred to `testkit::ArmedFaults`; the actual
  path is `testkit::faults::ArmedFaults` (testkit re-exports nothing at the
  root), which the tests use.
- `event-journal` remains a declared but unused dependency of the crate
  (declared by CMD-000); `Cargo.toml` is outside this task's file lease, so it
  was left untouched.
- Span instrumentation is deliberately deferred (design ruling 3).

## Fix report (review follow-up)

- **Fix commit:** `4455507` — `fix(command): name the rejected command_type and loop replay asserts [CMD-001B]`
- **Scope:** `agent-os/crates/command-coordinator/src/lib.rs`,
  `agent-os/crates/command-coordinator/tests/coordinator.rs` (the other two
  leased files are unchanged).

**Important — R1.4 message must name the offending type.** The unknown-type
rejection now formats the submitted value into the message:

```rust
"command_type {:?} is not registered in the coordinator"
```

and `unknown_command_type_rejects_without_rows` asserts
`error.message().contains("session.unknown")` before checking zero rows. The
value is routing metadata, not payload, so N1 still holds (`{:?}` escapes the
string; no payload bytes are ever interpolated).

**Minor — P1 repeat-replay coverage.** `identical_replay_returns_the_stored_outcome_without_changes`
now re-submits the identical envelope three times after the first commit; each
replay asserts the returned outcome equals the first and that the full
session/idempotency/outbox observation is unchanged from the previous call,
with the handler-call counter remaining at one.

**Verification (all from `agent-os/`, exit codes checked directly):**

```
$ cargo test -p command-coordinator        # 5 unit + 10 integration passed, 0 failed
$ cargo test --workspace                   # exit 0, 82 suites "test result: ok", 0 failed
$ cargo clippy --workspace --all-targets -- -D warnings   # exit 0, no diagnostics
$ cargo fmt --check                        # exit 0, no output
```

RED/GREEN note: the message assertion was written first and failed against the
old fixed string, then passed after the `format!` change. The looped replay
test passed both before and after (behavior was already correct; coverage was
thin). Real RED output:

```
$ cargo test -p command-coordinator --test coordinator unknown_command_type_rejects_without_rows
running 1 test
test unknown_command_type_rejects_without_rows ... FAILED

thread '...' panicked at crates/command-coordinator/tests/coordinator.rs:404:5:
message must name the unregistered command_type: command_type is not registered in the coordinator

test result: FAILED. 0 passed; 1 failed; 9 filtered out
```

Then, after re-applying the `format!` fix: `unknown_command_type_rejects_without_rows` and
`identical_replay_returns_the_stored_outcome_without_changes` both pass, 5 + 10 tests green.
