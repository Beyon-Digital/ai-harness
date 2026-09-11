# Task PST-002 — Define the transaction contract and the testkit mock

- **Status:** DONE_WITH_CONCERNS
- **Agent:** agent-pst002
- **Commit:** `7b3a52a` `feat(store): transaction contract and testkit mock [PST-002]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What was implemented

1. **Port surface (`kernel-store`).**
   - `types.rs`: `TxContext`, `DaemonEpoch`, `DaemonFence` exactly as fixed in
     `design.md` Interfaces.
   - `txn.rs`: object-safe `KernelStore`, `KernelTxn`, `KernelReadTxn` with the exact
     accessor lists from `design.md`; `begin_write`/`begin_read` return
     `Box<dyn ... + '_>`; `commit`/`rollback` consume `Box<Self>`.
   - `repositories.rs`: read/write trait split per D3 — every write trait extends its
     read trait and read traits expose no mutation. Run, task, session, graph,
     environment, effect, resource, timer, security, config, workspace, adapter,
     artifact, loop, idempotency, and stream groups; group-specific operations follow
     the design table literally (`cas_transition`, `ensure_head`, `insert_dependency`,
     `cas_head_revision`, `insert_bindings`, `cas_active`, `cas_lease`,
     `cas_instance_state`, `cas_turn`, `allocate`, `scan_unpublished`,
     `mark_published`, ...). No SQL type or SQL text crosses the port (R2.5).
   - `models.rs`: field-for-field row mirrors for all 28 named tables plus `New*`,
     `*Cas`, and `*Patch` types. Integer enums use `from_wire`-typed domain enums;
     TEXT state columns with domain converters use `from_state_str`-typed enums
     (`RunState`/`RecoveryDisposition` are integers in the DDL; `TimerState`,
     `ReservationState`, `LeaseEnforcementState`, `TrustState`, `ConformanceState`,
     `ApprovalState` are the schema-checked TEXT state domains).
   - `lib.rs`: module wiring and re-exports (`kernel_store::RunRow`, `NewRun`,
     `KernelTxn`, `DaemonFence`, ...).
2. **Object-safety test.** `kernel-store/src/txn.rs` test module boxes a stub `KernelTxn`
   and performs a no-op `context()` call through `Box<dyn KernelTxn>`, asserting the
   vtable is callable.
3. **Testkit mock.** `testkit/src/store.rs` implements the full contract in memory:
   - private per-transaction snapshot clone → commit swaps the snapshot into the
     store under the store lock, rollback/drop discards it (R2.3/R2.4);
   - optimistic commit guard: a transaction whose base revision changed fails with
     `Conflict`/`Safe` instead of overwriting;
   - CAS conditions mirror the storage checks (`RunCas` revision/state/cancellation
     epoch, effect state + fencing token, timer state + version, reservation state,
     lease epoch, adapter instance state, turn state);
   - idempotency `(principal, key)` uniqueness returns `Conflict` on duplicate
     insert; `lookup` replays the stored outcome;
   - stream allocation is in-transaction and contiguous per stream key; outbox
     uniqueness of `event_id` and `(stream_key, sequence)` is enforced; publication
     marks only the selected channel.
   - FK-shaped checks mirror constraints (run→task/parent, effect→run,
     environment/bindings, registration→instance/report, response→request).
4. **Contract test.** `testkit/tests/store_mock.rs` performs a representative
   session+task+run+effect+outbox+idempotency mutation in one mock write transaction
   and asserts nothing is visible pre-commit and everything is visible post-commit,
   plus drop rollback, CAS staleness, idempotent replay/duplicate conflict, and
   cross-transaction contiguous stream allocation.

## Acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| R2.1 / R2.5 — write txns expose typed repos, no SQL; traits object safe | **met** | `txn::tests::boxed_kernel_txn_performs_a_no_op_call` passes; no `sqlx`/SQL identifiers in `kernel-store/src` |
| R2.2 — read txns expose reads only (compile-time proof) | **met** | `KernelReadTxn` returns only `*Read` trait objects; no insert/CAS method is reachable from a read txn |
| R2.3 / R2.4 — mock commit atomic; unreported drops leave no state | **met** | `committed_mutations_are_visible_atomically`, `dropped_transaction_leaves_no_state` |
| R2.6 — mock implements the identical trait objects used by production | **met** | `MockStore: KernelStore`; `MockWriteTxn: KernelTxn`; `MockReadTxn: KernelReadTxn`; `tests/store_mock.rs` exercises them through `Box<dyn ...>` |
| No file outside `files:` changed | **met** | `git show --stat 7b3a52a` lists only the 8 leased paths |

## RED

Reproduced on a scratch copy of the parent commit with only the test artifacts present
(final `lib.rs` wiring, the `txn.rs` object-safety test module, `testkit/src/lib.rs`
with `pub mod store;`, and `tests/store_mock.rs`):

```
$ cargo test -p kernel-store -p testkit
error[E0583]: file not found for module `models`
 --> crates/kernel-store/src/lib.rs:4:1
error[E0583]: file not found for module `repositories`
 --> crates/kernel-store/src/lib.rs:5:1
error[E0583]: file not found for module `types`
 --> crates/kernel-store/src/lib.rs:7:1
error[E0432]: unresolved imports `txn::KernelReadTxn`, `txn::KernelStore`, `txn::KernelTxn`
  --> crates/kernel-store/src/lib.rs:11:15
error: could not compile `kernel-store` (lib) due to 4 previous errors
error[E0405]: cannot find trait `RunRepo` in the crate root
  --> crates/kernel-store/src/txn.rs:19:47
error[E0405]: cannot find trait `TaskRepo` in the crate root
  --> crates/kernel-store/src/txn.rs:23:48
error[E0405]: cannot find trait `SessionRepo` in the crate root
  --> crates/kernel-store/src/txn.rs:27:51
```

## GREEN / verification

```
$ cargo test -p kernel-store -p testkit
running 1 test
test txn::tests::boxed_kernel_txn_performs_a_no_op_call ... ok
test result: ok. 1 passed; 0 failed
running 8 tests
test result: ok. 8 passed; 0 failed        (existing testkit clock/faults/ids)
running 4 tests
test result: ok. 4 passed; 0 failed        (existing daemon_host)
running 1 test
test prop_faults ... ok
test result: ok. 1 passed; 0 failed
running 5 tests
test idempotency_replay_and_duplicate_keys_conflict ... ok
test cas_conditions_reject_stale_expectations ... ok
test committed_mutations_are_visible_atomically ... ok
test dropped_transaction_leaves_no_state ... ok
test stream_allocation_is_contiguous_across_transactions ... ok
test result: ok. 5 passed; 0 failed

$ cargo clippy -p kernel-store -p testkit --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)

$ cargo fmt --check -p kernel-store -p testkit
FMT OK (my packages)

$ cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s)
```

## Files changed (commit `7b3a52a`)

| Path | Change |
|---|---|
| `agent-os/crates/kernel-store/src/lib.rs` | modify: modules + re-exports |
| `agent-os/crates/kernel-store/src/types.rs` | create: `TxContext`, `DaemonEpoch`, `DaemonFence` |
| `agent-os/crates/kernel-store/src/txn.rs` | create: `KernelStore`/`KernelTxn`/`KernelReadTxn` + object-safety test |
| `agent-os/crates/kernel-store/src/repositories.rs` | create: read/write repo traits, `PublishKind` |
| `agent-os/crates/kernel-store/src/models.rs` | create: 28 row mirrors + `New*`/`*Cas`/`*Patch` |
| `agent-os/crates/testkit/src/store.rs` | create: `MockStore`, `MockWriteTxn`, `MockReadTxn` |
| `agent-os/crates/testkit/src/lib.rs` | modify: `pub mod store;` |
| `agent-os/crates/testkit/tests/store_mock.rs` | create: mock contract test |

## Concerns

1. **Repo-wide `cargo fmt --check` currently fails only in the parallel lease.**
   `crates/kernel-store-sqlite/src/schema.rs` and `tests/bootstrap.rs` (PST-001/003A
   in flight) have formatting diffs. `cargo fmt --check -p kernel-store -p testkit`
   is clean; the workspace-wide check is expected to clear when that agent formats
   its own files.
2. **Design gave operation names, not signatures, for several CAS methods.** The
   resolved ports used by PST-003A/B: `insert_dependency(new, expected_revision)`
   verifies the head revision, inserts with `created_graph_revision = expected_revision`,
   and advances the head by one; `cas_active(expected_revision, generation,
   activated_at_ms)`; `cas_lease(id, expect_epoch, patch)`; `cas_instance_state(id,
   expect_state, patch)` and `cas_turn(id, expect_state, patch)` take the persisted
   TEXT literal as `&str`.
3. **TEXT states with no foundation enum mirror as `String`** (loop turn state,
   adapter instance state, conformance result, config validation/test states,
   approval response decision, decision type). They carry a doc comment naming the
   DDL CHECK domain; if the foundation later adds enums, these fields should be
   retyped.
4. **`OutboxEventRow` mirrors all DDL columns, including `occurred_at_ms`.** The task
   brief listed `created_at_ms`; the DDL column is `occurred_at_ms`, so the DDL name
   was used for a field-for-field mirror (plus `event_version`, `run_id`, `task_id`,
   `session_id`, `effect_id`, which the brief's list omitted).
5. **Mock commit is optimistic.** Unlike SQLite `BEGIN IMMEDIATE`, two overlapping
   mock write transactions with the same base revision do not block; the loser gets
   `Conflict`/`Safe`. No contract test depends on blocking behaviour.
6. **Spec ledger/tasks status moves were not included in the task commit** (they are
   managed by spec-flow and were concurrently modified by the parallel agent); the
   commit contains only the 8 leased source paths.

---

## Fix report (review follow-up)

- **Fix commit:** `1b29de1` `fix(store): mirror daemon-epoch assertion and pin CAS semantics [PST-002]`
- **Findings addressed:** 1 (mock epoch assertion), 2 (whole-store revision limitation
  documented), 3 (graph head semantics pinned), plus the minor read-trait note.

### Finding 1 — `begin_write` now asserts the persisted daemon epoch

`MockState` gained a `daemon_epoch` field with documented initial value `0` (no fence
recorded) and `MockStore::set_daemon_epoch` as the test helper. `begin_write` rejects
an absent (`0`) or mismatched context epoch with `FailedPrecondition`, mirroring the
storage-layer assertion (R4.3, D6); `acquire_daemon_fence` now derives the next epoch
from the persisted value and updates it, so the helper and the fence path agree.

New contract test `begin_write_asserts_the_persisted_daemon_epoch` covers both paths:

```
running 6 tests
test begin_write_asserts_the_persisted_daemon_epoch ... ok
test dropped_transaction_leaves_no_state ... ok
test cas_conditions_reject_stale_expectations ... ok
test idempotency_replay_and_duplicate_keys_conflict ... ok
test committed_mutations_are_visible_atomically ... ok
test stream_allocation_is_contiguous_across_transactions ... ok
test result: ok. 6 passed; 0 failed
```

The existing fixture seeds epoch `1` via `set_daemon_epoch(1)`; the test asserts that
epoch `1` is refused before any fence, epoch `0` (absent) is refused, stale epoch `1`
is refused after `set_daemon_epoch(2)`, epoch `2` opens, and the epoch recorded by
`acquire_daemon_fence` (`3`) opens.

### Finding 2 — whole-store revision limitation documented

The `MockStore` struct doc and the module doc now state that commit is guarded by a
single whole-store revision: any overlapping write transaction started from an older
revision fails with `Conflict`/`Safe` even when CAS expectations match and the writes
are disjoint, unlike SQLite's `BEGIN IMMEDIATE` writer serialization. Mock-based
concurrency tests are told to stay conservative and not assume both overlapping
writers succeed. No architectural change was made.

### Finding 3 — graph head semantics pinned on the trait

`GraphRepo::cas_head_revision` now documents "increments `graph_revision` from
`expected` to `expected + 1` on success; `false` and unchanged when the head is
absent or the revision differs". `GraphRepo::insert_dependency` documents "requires an
existing head at `expected_revision` (`NotFound` when absent, `Conflict` on mismatch),
inserts with `created_graph_revision = expected_revision`, and advances the head to
`expected_revision + 1` in the same operation". PST-003A can mirror these exactly.

### Minor — write-only groups noted

`IdempotencyRepo` and `StreamRepo` now carry doc comments stating they are write-only
in the port surface because `KernelReadTxn` does not expose them (replay lookups,
stream allocation, and outbox staging happen inside write transactions), so there is
no read trait to extend.

### Verification after fixes

```
$ cargo test -p kernel-store -p testkit
test txn::tests::boxed_kernel_txn_performs_a_no_op_call ... ok
...
test result: ok. 6 passed; 0 failed        (store_mock contract tests)
test result: ok. 8 passed; 0 failed        (existing testkit tests)
test result: ok. 4 passed; 0 failed        (existing daemon_host)
test result: ok. 1 passed; 0 failed        (existing prop_faults)

$ cargo clippy -p kernel-store -p testkit --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)

$ cargo fmt --check
cargo fmt --check exit=0
```

### Files changed in `1b29de1`

| Path | Change |
|---|---|
| `agent-os/crates/kernel-store/src/repositories.rs` | doc: graph CAS/insert semantics; write-only group notes |
| `agent-os/crates/testkit/src/store.rs` | epoch field + helper, `begin_write` assertion, commit-limit docs |
| `agent-os/crates/testkit/tests/store_mock.rs` | fixture seeds epoch; new epoch-path contract test |

### Open concerns after fixes

- The concurrency limitation (Finding 2) is documented, not fixed by design; tests
  that need disjoint concurrent writers must use the SQLite implementation.
- `acquire_daemon_fence` sets `lease_expires_unix_ms = i64::MAX` as a mock sentinel;
  lease-expiry semantics remain the SQLite store's responsibility (PST-004).

---

## Fix report 2 (scoped re-review follow-up)

- **Fix commit:** `15cffa3` `fix(store): version fence changes and keep fence state consistent [PST-002]`
- **Findings addressed:** Important (fence acquisition did not version the store;
  an in-flight write could restore a stale epoch/fence on commit) and minor
  (`set_daemon_epoch` left `current_fence()` empty).

### Important — fence changes now version the store

A private `install_fence` helper persists the epoch + fence and bumps
`MockState::revision`; both `acquire_daemon_fence` and the seeding helper go
through it. A write transaction opened before a fence acquisition therefore
holds an older `base_revision`, and `MockWriteTxn::commit` fails with
`Conflict`/`Safe` **before** replacing the whole snapshot, so the new
epoch/fence survives and late stale-epoch writes stay rejected (R4.3/D6).

New contract test `fence_acquisition_invalidates_in_flight_write` proves the
exact sequence: open at epoch N, acquire fence to N+1, old commit returns
`Conflict`, `begin_write(N)` is rejected with `FailedPrecondition`,
`begin_write(N+1)` opens, and `current_fence()` still reports N+1.

```
running 7 tests
test begin_write_asserts_the_persisted_daemon_epoch ... ok
test fence_acquisition_invalidates_in_flight_write ... ok
test dropped_transaction_leaves_no_state ... ok
test cas_conditions_reject_stale_expectations ... ok
test idempotency_replay_and_duplicate_keys_conflict ... ok
test committed_mutations_are_visible_atomically ... ok
test stream_allocation_is_contiguous_across_transactions ... ok
test result: ok. 7 passed; 0 failed
```

### Minor — `set_daemon_epoch` keeps fence state consistent

`set_daemon_epoch(epoch, instance)` now installs a matching `DaemonFence` (and
bumps the revision); passing `0` clears the fence. `current_fence()` can no
longer return `None` while a nonzero epoch opens writes. The fixture and the
epoch-assertion test pass an instance id accordingly, and both tests assert
the fence matches the epoch that opened the transaction.

### Verification after fixes

```
$ cargo test -p kernel-store -p testkit
test txn::tests::boxed_kernel_txn_performs_a_no_op_call ... ok
test result: ok. 1 passed; 0 failed
test result: ok. 8 passed; 0 failed        (existing testkit tests)
test result: ok. 4 passed; 0 failed        (existing daemon_host)
test result: ok. 1 passed; 0 failed        (existing prop_faults)
test result: ok. 7 passed; 0 failed        (store_mock contract tests)

$ cargo clippy -p kernel-store -p testkit --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)

$ cargo fmt --check
cargo fmt --check exit=0
```

### Files changed in `15cffa3`

| Path | Change |
|---|---|
| `agent-os/crates/testkit/src/store.rs` | `install_fence` versions the store; helper installs/clears a real fence |
| `agent-os/crates/testkit/tests/store_mock.rs` | new in-flight/write-fence contract test; helper refactor |

### Open concerns after fix 2

- None new. The whole-store optimistic revision remains the documented
  concurrency limitation from fix 1 (unchanged by design).
