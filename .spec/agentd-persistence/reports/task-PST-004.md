# Task PST-004 — Daemon lock, fence, and instance lifecycle

- **Status:** DONE_WITH_CONCERNS
- **Owner:** agent-pst004
- **Commit:** `33bd799` — `feat(store): daemon lock and durable fence [PST-004]`
- **Date:** 2026-09-12
- **Depends on:** PST-003B (satisfied)
- **Branch:** `feat/agentd-microkernel-mvp`

## Implementation notes

Files (lease-scoped, nothing else changed by this task):

| File | Role |
|---|---|
| `crates/kernel-store-sqlite/src/fence.rs` | `claim`, `current`, `assert_epoch`, `FENCE_LEASE_MS` |
| `crates/kernel-store-sqlite/src/lib.rs` | `pub mod fence;` |
| `crates/kernel-store-sqlite/src/txn.rs` | `acquire_daemon_fence`/`current_fence` delegate to `fence`; `begin_write` calls `fence::assert_epoch` |
| `crates/kernel-store-sqlite/tests/fence.rs` | 5 fence acceptance tests |
| `crates/agentd/src/lock.rs` | `DaemonLock`, `LockError`, `LOCK_FILE_NAME` |
| `crates/agentd/tests/lock_exclusion.rs` | self-exec two-process exclusion test |
| `crates/identity/src/lib.rs` | `pub mod daemon;` |
| `crates/identity/src/daemon.rs` | `DaemonHandle`, `acquire` |

### Semantics

- **Fence claim** runs in an explicit `BEGIN IMMEDIATE` on its own pooled
  connection, so concurrent claimants serialize on SQLite's write lock. The
  first claim inserts epoch `1`; later claims use
  `ON CONFLICT(singleton) DO UPDATE ... fencing_epoch = fencing_epoch + 1`
  and return the stored row. `lease_expires_ms = now + lease_ms` and
  `updated_at_ms = now`; the previous placeholder `i64::MAX` lease is gone.
  The production trait path uses `FENCE_LEASE_MS = 15_000`
  (`daemon.fence_lease_ms` in `specs/limits.yaml`).
- **Epoch assertion.** The guard's admission check moved verbatim into
  `fence::assert_epoch`; `begin_write` now calls it between
  `BEGIN IMMEDIATE` and handing the transaction to repositories. Behaviour is
  unchanged (absent or stale epoch → `FailedPrecondition`/`Never`, zero
  mutations), so the PST-003A assertion is preserved while `fence.rs` owns
  claim/read/assert as designed.
- **`DaemonLock`** opens `<runtime_dir>/agentd.lock` with
  `OpenOptions::create(true).truncate(false).mode(0o600)` and calls
  `std::fs::File::try_lock` (no new dependency, D7). `WouldBlock` maps to
  `LockError::Held { path }`; `release` is an explicit best-effort `unlock`
  plus drop. Process death releases the OS lock, so no stale lock survives a
  crash.
- **`identity::daemon::acquire(store, ids, lease_ms)`** mints a
  `DaemonInstanceId` from the caller-supplied `IdProvider`, claims through
  `KernelStore::acquire_daemon_fence`, and returns
  `DaemonHandle { instance_id, fence }`. `lease_ms` is validated as positive
  before the claim; the durable lease is the store's normative value (see
  concern 1).
- **Two-process test** uses the self-exec pattern: with
  `AGENTD_LOCK_TEST_CHILD` set, the test binary acquires the lock, prints
  `LOCK_READY`, and serves `ping`/`LOCK_PONG` over stdin/stdout. The parent
  waits for readiness by blocking on the child's stdout (no sleeps, N2),
  proves `DaemonLock::acquire` returns `Held` with the exact path, pings the
  holder to prove it is unaffected, verifies the lock file is `0600`, verifies
  a sentinel `kernel.db` is byte-identical, then kills the child and proves
  the lock is immediately re-acquirable and releasable.

## Acceptance criteria

| Criterion | Evidence | Command |
|---|---|---|
| R4.1 / R4.6 — second process refuses; first unaffected; database untouched | `lock_exclusion::second_process_is_refused_and_kill_releases_the_lock`: `Held { path }`, `ping`/`pong` liveness, sentinel `kernel.db` bytes unchanged, mode `0600` | `cargo test -p agentd --test lock_exclusion` |
| R4.2 / R4.5 — claims strictly increasing across restarts | `fence::successive_claims_strictly_increase_the_epoch`, `fence::restart_keeps_the_fence_and_increments_the_next_claim`, `fence::concurrent_claims_serialize_into_distinct_epochs` | `cargo test -p kernel-store-sqlite --test fence` |
| R4.3 — stale-epoch transaction rejected without mutation | `fence::stale_epoch_transaction_is_rejected_without_mutation` (commits one session under epoch 1, supersedes to epoch 2, replays epoch 1 → `FailedPrecondition`/`Never`, row count and persisted epoch unchanged) | same |
| R4.4 — losing the fence refuses new transactions | Exercised by the stale path above; documented in `fence.rs` module docs (persisted row is the single authority, D8) | same |
| N2 — no sleeps; readiness over the child's stdout | `wait_for_line` blocks on the pipe; `lock_exclusion` finishes in ~0.24 s; no `sleep` anywhere in the changed files | `cargo test -p agentd --test lock_exclusion` |
| No file outside `files:` changed | Commit contains exactly the 8 leased paths | `git show --stat 33bd799` |

## RED / GREEN

RED — `cargo test -p kernel-store-sqlite --test fence` before implementation
(the three behavioural tests already passed against PST-003A's minimal logic;
the durable-lease assertion failed on the placeholder):

```
running 4 tests
test first_claim_creates_epoch_one_with_a_live_lease ... FAILED
test stale_epoch_transaction_is_rejected_without_mutation ... ok
test successive_claims_strictly_increase_the_epoch ... ok
test restart_keeps_the_fence_and_increments_the_next_claim ... ok

thread 'first_claim_creates_epoch_one_with_a_live_lease' panicked at crates/kernel-store-sqlite/tests/fence.rs:80:5:
assertion `left != right` failed: placeholder lease survived the claim
  left: 9223372036854775807
 right: 9223372036854775807

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s
```

GREEN — `cargo test -p kernel-store-sqlite --test fence`:

```
running 5 tests
test stale_epoch_transaction_is_rejected_without_mutation ... ok
test first_claim_creates_epoch_one_with_a_live_lease ... ok
test restart_keeps_the_fence_and_increments_the_next_claim ... ok
test concurrent_claims_serialize_into_distinct_epochs ... ok
test successive_claims_strictly_increase_the_epoch ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
```

GREEN — `cargo test -p agentd --test lock_exclusion` (run directly as its own
target, not only in the suite):

```
running 1 test
test second_process_is_refused_and_kill_releases_the_lock ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.24s
```

## Verification

From `agent-os/`:

- `cargo test -p kernel-store-sqlite -p agentd -p identity` — all pass:
  kernel-store-sqlite 6 unit + 8 bootstrap + 6 cas + 2 contention + 5 fence +
  4 immutability + 12 repos_remaining + 7 rollback; agentd lock_exclusion 1;
  identity 0 (no tests declared); doc-tests 0.
- `cargo clippy -p kernel-store-sqlite -p agentd -p identity --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean (whole workspace).
- `rg "unwrap\(|expect\(|sleep" src/fence.rs src/txn.rs src/lock.rs src/daemon.rs`
  from the crate roots — no matches outside test files; the lock test has no
  sleeps.

## Concerns

1. **`lease_ms` cannot reach the store through the frozen port.**
   `KernelStore::acquire_daemon_fence(instance)` takes no lease, while the
   design interface for `identity::daemon::acquire` takes `lease_ms`. The
   durable lease is therefore the store's normative `FENCE_LEASE_MS` (15 s
   from `specs/limits.yaml`), and identity validates only that the requested
   lease is positive. If callers must control the lease, `kernel-store`'s port
   signature has to grow a parameter (outside this task's lease).
   `daemon.fence_renew_ms` remains unconsumed because renewal is not part of
   PST-004.
2. **The lock test cannot claim a fence.** `agentd` declares no
   dev-dependencies and `agentd/Cargo.toml` is outside this task's file lease,
   so `lock_exclusion.rs` cannot open a `kernel.db`. The context's "epoch
   increments on the next claim" after killing the child is covered by
   `tests/fence.rs::restart_keeps_the_fence_and_increments_the_next_claim`
   (and the concurrent-claims test); the two-process test asserts the OS
   release (`acquire` succeeds immediately after `kill` + `wait`).
3. **`lock.rs` carries `#![allow(dead_code)]`.** The placeholder composition
   root (`agentd/src/main.rs`) does not consume the lock yet and is outside the
   lease, so the private `mod lock;` makes the binary target report every item
   as dead code, failing the `-D warnings` gate. The two-process test is the
   live consumer; the attribute is scoped to this module only.
4. **The lock and the store are not yet wired together.** `DaemonLock` is a
   standalone startup gate; R4.6's "database untouched by the second process"
   is demonstrated with a sentinel `kernel.db` and the store is never opened
   by the refused path. Composition-root wiring is a later task.
5. **Duplicated lease validation.** Positivity is checked both in
   `identity::daemon::acquire` (caller-side, keeps `lease_ms` meaningful) and
   in `fence::claim` (storage-side, guards the public function). Intentional
   defense in depth; worth collapsing if the port ever carries the lease.
