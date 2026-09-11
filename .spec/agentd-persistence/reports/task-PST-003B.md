# Task PST-003B — Remaining entity repositories

- **Status:** DONE_WITH_CONCERNS
- **Owner:** agent-pst003b
- **Commit:** `44aa197` — `feat(store): remaining entity repositories [PST-003B]`
- **Date:** 2026-09-12
- **Depends on:** PST-003A (satisfied)
- **Branch:** `feat/agentd-microkernel-mvp`

## Implementation notes

Files (lease-scoped, nothing else changed by this task):

| File | Role |
|---|---|
| `src/repos/adapters.rs` | `AdapterRead`/`AdapterRepo`: immutable registrations and conformance reports, adapter instance insert, state CAS |
| `src/repos/artifacts.rs` | `ArtifactRead`/`ArtifactRepo`: immutable insert, get-by-id, get-by-uri |
| `src/repos/config.rs` | `ConfigRead`/`ConfigRepo`: generations, singleton active pointer CAS |
| `src/repos/effects.rs` | `EffectRead`/`EffectRepo`: insert, get, list-by-run, `(state, token)` CAS |
| `src/repos/loop_turns.rs` | `LoopRead`/`LoopRepo`: turns, turn-state CAS, decisions |
| `src/repos/resources.rs` | `ResourceRead`/`ResourceRepo`: insert, get, list-by-run, state CAS |
| `src/repos/security.rs` | `SecurityRead`/`SecurityRepo`: grants, delegation hops, approval requests/responses |
| `src/repos/timers.rs` | `TimerRead`/`TimerRepo`: insert, get, due listing, `(state, version)` CAS that bumps `version` |
| `src/repos/workspaces.rs` | `WorkspaceRead`/`WorkspaceRepo`: workspaces, leases, epoch CAS |
| `src/repos/mod.rs` | nine `pub(crate) mod` declarations; `UnavailableRepo` shim trimmed to `IdempotencyRepo` and `StreamRepo` (PST-005) |
| `src/txn.rs` | `SqliteWriteTxn`/`SqliteReadTxn` own real views for the nine groups; idempotency/streams stay on the shim |
| `tests/repos_remaining.rs` | 10 tests: one CRUD+CAS test per group plus unknown persisted-value decoding |

### Semantics

- SQL is one runtime-parameterized statement per operation (constant SELECT
  fragments are assembled with `format!` from a `const &str`; no value is ever
  interpolated). Patch columns use `COALESCE(?n, column)`; a CAS returns
  `rows_affected() == 1` as `bool` and a mismatched expectation mutates
  nothing (R3.2).
- CAS expectations: effects `(state, executor_fencing_token)` with the token
  checked only when `expect_token` is `Some`; resources `(state)`; timers
  `(state, version)` and `version = version + 1` on success; config active
  pointer `expected_revision` (zero when absent, `NotFound` for a missing
  generation); workspace leases `lease_epoch`; adapter instances `state`; loop
  turns `state`.
- The config pointer CAS reads the current revision, compares it, verifies the
  target generation exists, then upserts `revision + 1`. Because every writer
  holds `BEGIN IMMEDIATE`, the read-then-write cannot interleave with another
  writer (R3.3).
- Immutable groups expose insert/read only: capability grants, delegation hops,
  approval requests (trigger-protected), adapter registrations, conformance
  reports (trigger-protected), and artifacts.
- Mapping is shared: every decode goes through `mapping::{decode_id,
  decode_opt_id, decode_u64, decode_wire, decode_state, text, opt_text, int,
  opt_int, blob, opt_blob}`; every sqlx failure goes through
  `mapping::from_sqlx`. No SQL type or sqlx error crosses the port (R3.1,
  R3.4). `delegation_hops.hop_index` additionally range-checks into `u32`.
- Behaviour mirrors the PST-002 testkit mock operation-for-operation,
  including `updated_at_ms` left unchanged by CAS operations (PST-003A concern
  4) and timers advancing `version` on every successful transition.

## Acceptance criteria

| Criterion | Evidence | Command |
|---|---|---|
| R3.1 stored values decode through domain types and fail closed | `repos_remaining::unknown_persisted_values_fail_closed` (wire `effects.state = 99` and TEXT `timers.state = 'bogus'` read back as `Internal`/`Never`); every decode in the nine files uses `mapping::` helpers | `cargo test -p kernel-store-sqlite` |
| R3.2 every CAS reports conflicts without mutation | `effect_repo_crud_and_cas` (stale state, stale token), `resource_repo_crud_and_cas`, `timer_repo_crud_and_cas` (stale version, version bump), `config_repo_crud_and_cas_active` (stale revision), `workspace_repo_crud_and_cas_lease` (stale epoch), `adapter_repo_crud_and_cas_instance_state` (stale state), `loop_turn_repo_crud_and_cas` (stale state) | same |
| R3.3 reads and writes respect transaction isolation | All CAS reads and writes run on the `BEGIN IMMEDIATE` connection via `SharedConn`; no test observes a partial CAS (each conflicting CAS leaves the earlier value asserted after commit) | same |
| R3.4 constraint failures map to stable codes | `artifact_repo_crud` duplicate URI → `Conflict`/`Never`; `config_repo_crud_and_cas_active` missing generation → `NotFound`/`Never`; PK/unique/FK/CHECK mapping unit-covered by PST-003A `mapping::tests` | same |
| No file outside `files:` changed | commit `44aa197` contains exactly the 12 leased paths | `git show --stat 44aa197` |
| Gates | clippy `-D warnings` clean, `cargo fmt --check` clean | see Verification |

All 43 crate tests pass: 6 unit + 8 bootstrap + 6 cas + 2 contention + 4
immutability + 10 repos_remaining + 7 rollback.

## RED / GREEN

RED — `cargo test -p kernel-store-sqlite --test repos_remaining` before the
implementation (compiles against the fail-closed shim, fails at runtime):

```
running 10 tests

thread 'resource_repo_crud_and_cas' panicked at tests/repos_remaining.rs:242:10:
called `Result::unwrap()` on an `Err` value: KernelError { code: FailedPrecondition, retry: Never,
message: "repository group ResourceRepo is not available", source: None }

thread 'security_repo_crud' panicked ... "repository group SecurityRepo is not available"
thread 'timer_repo_crud_and_cas' panicked ... "repository group TimerRepo is not available"
thread 'unknown_persisted_values_fail_closed' panicked ... "repository group EffectRepo is not available"
thread 'workspace_repo_crud_and_cas_lease' panicked ... "repository group WorkspaceRepo is not available"

failures:
    adapter_repo_crud_and_cas_instance_state
    artifact_repo_crud
    config_repo_crud_and_cas_active
    effect_repo_crud_and_cas
    loop_turn_repo_crud_and_cas
    resource_repo_crud_and_cas
    security_repo_crud
    timer_repo_crud_and_cas
    unknown_persisted_values_fail_closed
    workspace_repo_crud_and_cas_lease

test result: FAILED. 0 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s
```

GREEN — `cargo test -p kernel-store-sqlite` from `agent-os/`:

```
running 6 tests   (unittests src/lib.rs)
test result: ok. 6 passed; 0 failed

running 8 tests   (tests/bootstrap.rs)
test result: ok. 8 passed; 0 failed

running 6 tests   (tests/cas.rs)
test result: ok. 6 passed; 0 failed

running 2 tests   (tests/contention.rs)
test result: ok. 2 passed; 0 failed

running 4 tests   (tests/immutability.rs)
test result: ok. 4 passed; 0 failed

running 10 tests  (tests/repos_remaining.rs)
test adapter_repo_crud_and_cas_instance_state ... ok
test artifact_repo_crud ... ok
test config_repo_crud_and_cas_active ... ok
test effect_repo_crud_and_cas ... ok
test loop_turn_repo_crud_and_cas ... ok
test resource_repo_crud_and_cas ... ok
test security_repo_crud ... ok
test timer_repo_crud_and_cas ... ok
test unknown_persisted_values_fail_closed ... ok
test workspace_repo_crud_and_cas_lease ... ok
test result: ok. 10 passed; 0 failed

running 7 tests   (tests/rollback.rs)
test result: ok. 7 passed; 0 failed

running 0 tests   (Doc-tests)
test result: ok. 0 passed; 0 failed
```

## Verification

- `cargo test -p kernel-store-sqlite` — 43 passed, 0 failed.
- `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean (whole workspace).
- `rg "unwrap()|expect(|sleep" src/repos/{adapters,artifacts,config,effects,loop_turns,resources,security,timers,workspaces}.rs src/repos/mod.rs src/txn.rs` — no matches outside tests; tests contain no sleeps.

## Concerns

1. **Workspace lease CAS checks `lease_epoch`, not `mode`.** The task context
   phrases the expectation as `(mode, epoch)` but the frozen port signature is
   `cas_lease(id, expect_epoch, patch)` and the PST-002 mock checks only the
   epoch. The store keeps mock parity. The `mode` dimension is still
   protected by the schema's partial unique index
   `uq_workspace_exclusive_lease(workspace_id) WHERE mode = 2 AND
   enforcement_state = 'active'`, which surfaces as `Conflict`. If the reviewer
   intended an additional mode expectation, the port signature must grow a
   parameter; this task cannot change `kernel-store`.
2. **`loop_turns.state` and `adapter_instances.state` are raw `String` in the
   port models.** Unknown values in those columns therefore cannot fail
   decoding; the fail-closed test uses domain-typed columns (wire enum on
   `effects.state`, state string on `timers.state`). This matches
   `models.rs` and the mock.
3. **`updated_at_ms` is not advanced by any CAS.** No patch type carries a
   timestamp, and the mock leaves the column at its insert value; parity is
   deliberate (same as PST-003A concern 4).
4. **`TimerRepo::list_due` filters on `due_at_ms` only.** It returns timers in
   any state up to the cutoff, exactly like the mock; callers filter by state.
5. **Trigger-protected tables have no new trigger test here.** Approval
   requests and conformance reports are immutable by DDL trigger, but no port
   operation updates them, so there is nothing to drive through the port
   (same shape as PST-003A concern 6; `classify_sqlite(1811)` already covers
   the mapping).
6. **`cas_active` uses a read-then-upsert inside the immediate transaction.**
   Correct under `BEGIN IMMEDIATE`, but the mock's whole-store-revision
   semantics differ for overlapping writers; no cross-store test compares the
   two directly.

## Post-review fix (findings 1 and 2)

- **Commit:** `b0ee13c` — `fix(store): pin lease exclusivity and validate text literals [PST-003B]`
- **Scope:** `src/repos/loop_turns.rs`, `src/repos/adapters.rs`,
  `src/repos/config.rs`, `tests/repos_remaining.rs`; no other files.
- **Ruling recorded by the controller:** the port signature stays frozen;
  epoch-only lease CAS plus `uq_workspace_exclusive_lease` is the intended
  shape.

### Finding 1 — exclusivity path pinned by test

`exclusive_lease_index_rejects_second_active_lease` seeds a workspace and an
active `EXCLUSIVE_WRITE` lease, then inserts a second active lease for the same
workspace and asserts the partial unique index maps to `Conflict`/`Never`.
It then proves the rejected insert mutated nothing by revoking the first lease
through the epoch CAS and successfully inserting the second one, and reads both
leases back after commit (`revoked` / `active`). The test also documents the
controller ruling: epoch-only CAS is the port shape and the index is the `mode`
guarantee.

### Finding 2 — raw TEXT literals now fail closed on decode

Exact DDL literal sets are validated on decode with `mapping::decode_state`
(returning `UnknownStateValue` on anything else, so the error is
`Internal`/`Never`):

| Column | Validator location | Literals |
|---|---|---|
| `loop_turns.state` | `repos/loop_turns.rs` | `issued`, `accepted`, `stale` |
| `decisions.decision_type` | `repos/loop_turns.rs` | `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval` |
| `adapter_instances.state` | `repos/adapters.rs` | `starting`, `ready`, `exited`, `failed` |
| `config_generations.validation_state` | `repos/config.rs` | `proposed`, `validated`, `rejected` |
| `config_generations.test_state` | `repos/config.rs` | `untested`, `passed`, `failed` |

`AdapterRead` has no instance getter, so `cas_instance_state` now reads the
persisted row and validates its state before issuing the update; a missing row
still returns `Ok(false)` (mock parity) and the update keeps its `WHERE
state = ?` predicate. This is the only decode path for `adapter_instances.state`
in the port; a future instance read must reuse `instance_state_from_state`.

`unknown_text_literals_fail_closed` seeds a turn, decision, adapter instance,
and two config generations, corrupts each target column with `PRAGMA
ignore_check_constraints` on a direct connection, reopens the store, and
asserts every read fails with `Internal`/`Never` (validation and test states
are corrupted on separate generations so both validators are exercised).

### Minor

`ConfigRepo::cas_active` now carries a comment stating that the read-then-upsert
is race-free only because every writer holds `BEGIN IMMEDIATE` on the shared
connection.

### Fix evidence (from `agent-os/`)

```
cargo test -p kernel-store-sqlite
running 6 tests   (unittests)              test result: ok. 6 passed
running 8 tests   (tests/bootstrap.rs)     test result: ok. 8 passed
running 6 tests   (tests/cas.rs)           test result: ok. 6 passed
running 2 tests   (tests/contention.rs)    test result: ok. 2 passed
running 4 tests   (tests/immutability.rs)  test result: ok. 4 passed
running 12 tests  (tests/repos_remaining.rs) test result: ok. 12 passed
running 7 tests   (tests/rollback.rs)      test result: ok. 7 passed
```

- `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.

### Residual concern

`adapter_instances.state` is only validated on the `cas_instance_state`
pre-read because the frozen port exposes no instance query; the same helper
must be wired into any future read method. Unknown literals already present in
the other raw TEXT columns not listed in the finding (`approval_responses.
decision`, `conformance_reports.result`) remain opaque `String`s in
`models.rs`; they are outside the reviewed scope but follow the same pattern if
the controller wants the full DDL CHECK set validated.
