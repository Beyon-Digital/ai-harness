# Tasks — agentd-persistence

**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)
**Design:** `design.md` (approved)

## Global constraints

- All Rust lives under `agent-os/`; no build-pack edits in this module.
- From `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- From the repo root: `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`
  and `python3 tools/validate_repo.py`.
- No `unwrap()`/`expect()` outside tests and proven startup invariants; no SQL escape hatch on
  the port; no secret or payload bytes in errors or logs; no wall-clock sleeps in tests.
- Every authoritative value comes from `design.md`, `requirements.md`, or the build-pack schemas.
- Commit per task with the task id in the message; retry `git commit` on `index.lock` after two
  seconds, up to five times.

## Execution contract for subagents

1. Claim before writing: `python3 "$SPECFLOW" claim agentd-persistence TASK_ID AGENT_ID` then `start`.
2. Stay inside the task's `files:` lease; `block` and report if another file is needed.
3. `depends_on` interfaces are contracts from `design.md`; do not redesign them.
4. Test first where the task says so; run the focused test, then the affected suite once.
5. Report `DONE` · `DONE_WITH_CONCERNS` · `BLOCKED` · `NEEDS_CONTEXT` honestly.
6. Never dispatch your own reviewer.
7. Commit scoped to your files, then `review`, then write
   `.spec/agentd-persistence/reports/task-TASK_ID.md`.

---

### Task PST-000: Declare persistence-module dependencies

- status: done
- owner: agent-pst000
- depends_on: none
- files: `agent-os/crates/kernel-store/Cargo.toml`, `agent-os/crates/kernel-store-sqlite/Cargo.toml`, `agent-os/crates/testkit/Cargo.toml`, `agent-os/crates/identity/Cargo.toml`, `agent-os/crates/events/Cargo.toml`, `agent-os/Cargo.lock`
- requirements: N3, G1
- scope: small
- model: cheap

**Objective:** Every persistence task compiles against declared dependencies, with the manifests owned by exactly one task.

**Context the implementer cannot infer:**

- FND-001 intentionally declared minimal per-crate dependencies; the persistence module needs more, and no other task may edit manifests.
- Required edits (use `.workspace = true` where the dependency is already in `[workspace.dependencies]`; add workspace entries only if missing):
  - `kernel-store`: add `async-trait`.
  - `kernel-store-sqlite`: add `kernel-store`, `domain`, `errors`, `sqlx` with `sqlite` and `runtime-tokio` features, `tokio` with `rt`, `async-trait`, `observability`; dev-dependencies `testkit`, `tempfile`, `proptest`, `tokio` with `macros` and `rt-multi-thread`.
  - `testkit`: add `kernel-store`, `errors`, `async-trait`; dev-dependency `tokio` with `macros` and `rt-multi-thread`.
  - `identity`: add `kernel-store`, `domain`, `errors`.
  - `events`: add `kernel-store`, `domain`, `errors`.
- Do not remove or reorder existing dependency entries; append only.
- The workspace manifest already declares `sqlx`, `tokio`, `async-trait`, `tempfile`, `proptest`. Path member crates are referenced directly with `path = "../name"`.

**Steps:**

- [ ] Edit the five manifests
- [ ] Run `cargo check --workspace` and `cargo test --workspace --no-run` from `agent-os/` to resolve the lock
- [ ] Run `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `chore(workspace): declare persistence module dependencies [PST-000]`

**Acceptance criteria:**

- [ ] Each crate resolves the dependencies its tasks need; `Cargo.lock` updated
- [ ] G1 — full workspace check, tests, clippy, and fmt still pass
- [ ] N3 — no contract, schema, or build-pack file changed
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo check --workspace && cargo test --workspace --no-run` exits 0
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

---

### Task PST-001: Bootstrap the kernel database

- status: done
- owner: agent-pst001
- depends_on: PST-000
- files: `agent-os/crates/kernel-store-sqlite/src/lib.rs`, `agent-os/crates/kernel-store-sqlite/src/schema.rs`, `agent-os/crates/kernel-store-sqlite/tests/bootstrap.rs`
- requirements: R1.1, R1.2, R1.3, R1.4, R1.5, N1, N3, G1, G2
- scope: medium
- model: standard
- blocked_reason: need agent-os/crates/kernel-store-sqlite/Cargo.toml: task requires sqlx (SqliteConnectOptions) and tokio/tempfile dev-deps for #[tokio::test]; manifest currently declares only domain/errors/kernel-store and sqlx is absent from Cargo.lock; cannot compile RED or GREEN without it

**Objective:** `SqliteKernelStore::open` creates `kernel.db` exactly once from the inception schema, seeds and verifies the schema version, applies the normative PRAGMAs and file modes, and never lets repositories create structure.

**Context the implementer cannot infer:**

- The schema source is `agent-os/schema/kernel_store.sql`, installed verbatim by FND-002; embed it with `include_str!`. It already contains the PRAGMAs, 30 tables, CHECKs, triggers, and the `kernel_meta` table; do not edit it.
- `StoreConfig { path, pool_max_connections, busy_timeout_ms }` and `SqliteKernelStore::open` are specified in `design.md` under kernel-store-sqlite. `open` returns `crate::Result` from `errors`; map sqlx open errors to `Unavailable` with `RetryClass::Safe`, and version mismatches to `FailedPrecondition` with `Never`.
- Use `sqlx::sqlite::SqliteConnectOptions` with `create_if_missing(false)` after you create the file yourself with mode `0600` (`std::os::unix::fs::PermissionsExt`); create the runtime directory `0700`. Apply `busy_timeout` from config and `journal_mode=WAL`, `synchronous=FULL`, `foreign_keys=ON` via connect options or PRAGMA statements. The schema file's own PRAGMA statements are the normative set; running the schema after opening with matching options is the simplest correct order.
- Version rule: read `kernel_meta` for key `schema_version`; on a fresh database the schema seeds it; if the value is absent or not exactly the text `1`, fail closed naming what was found. Do not create the row in repository code.
- Keep `lib.rs` minimal: the store struct, `open`, `path()`, and `pub mod schema;`. Later tasks add transaction modules.
- Tests in `tests/bootstrap.rs`: fresh bootstrap creates the file and tables; reopen is a no-op structurally; a manually corrupted version fails closed; a probe insert violating a CHECK and one violating a UNIQUE both fail; file mode is `0600` and directory `0700`.

**Steps:**

- [ ] Write the failing bootstrap test first: open on a temp dir, assert file exists with mode `0600`, `kernel_meta.schema_version == '1'`, and one known table exists
- [ ] Run `cargo test -p kernel-store-sqlite --test bootstrap` and confirm RED (missing API)
- [ ] Implement `schema.rs` and `lib.rs`
- [ ] Re-run and confirm GREEN; add the wrong-version, constraint, and mode cases
- [ ] Run `cargo test -p kernel-store-sqlite` and `cargo fmt --check`
- [ ] Commit: `feat(store): versioned kernel.db bootstrap [PST-001]`

**Acceptance criteria:**

- [ ] R1.1 — fresh bootstrap creates the database from the schema verbatim and seeds version 1
- [ ] R1.2 — reopen performs no structural change (no `CREATE` executed)
- [ ] R1.3 — wrong or missing version fails closed naming the value
- [ ] R1.4 — no repository or open path auto-creates tables
- [ ] R1.5 / N1 — PRAGMAs and `0600`/`0700` modes verified by test
- [ ] G2 — pack validators still pass; no pack file changed
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store-sqlite --test bootstrap` passes, output pristine
- [ ] `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` clean
- [ ] From the repo root: `python3 tools/validate_repo.py` prints `OK`

---

### Task PST-002: Define the transaction contract and the testkit mock

- status: done
- owner: agent-pst002
- depends_on: PST-000
- files: `agent-os/crates/kernel-store/src/lib.rs`, `agent-os/crates/kernel-store/src/types.rs`, `agent-os/crates/kernel-store/src/txn.rs`, `agent-os/crates/kernel-store/src/repositories.rs`, `agent-os/crates/kernel-store/src/models.rs`, `agent-os/crates/testkit/src/store.rs`, `agent-os/crates/testkit/src/lib.rs`, `agent-os/crates/testkit/tests/store_mock.rs`
- requirements: R2.1, R2.2, R2.3, R2.4, R2.5, R2.6, P2
- scope: large
- model: capable
- blocked_reason: need agent-os/crates/kernel-store/Cargo.toml and agent-os/crates/testkit/Cargo.toml: kernel-store needs async-trait.workspace = true for the #[async_trait] traits; testkit/src/store.rs must implement those traits (needs kernel-store + errors + async-trait deps) and tests/store_mock.rs needs tokio.workspace = true as dev-dep for #[tokio::test]. Without these manifest edits the task cannot compile RED or GREEN. All deps are in the local cargo cache and the registry is reachable, so a lock update + build succeeds once the manifests are amended. This is the same blocker PST-001 reported for kernel-store-sqlite/Cargo.toml.

**Objective:** The port crate publishes object-safe async traits, typed models, and no SQL access — plus an in-memory mock so the contract is provably implementable twice.

**Context the implementer cannot infer:**

- Exact trait signatures, `TxContext`, `DaemonEpoch`, `DaemonFence`, the read/write trait split, and the group-specific operations are all fixed in `design.md` Interfaces. Follow them exactly; write traits extend read traits, and read traits expose no mutation.
- `models.rs` mirrors every table column field-for-field using domain IDs and enums. Row types: TaskRow, SessionRow, RunRow, RunGraphHeadRow, RunDependencyRow, ResolvedEnvironmentRow, ResolvedBindingRow, AgentSpecRow, WorkspaceRow, WorkspaceLeaseRow, EffectRow, ReservationRow, TimerRow, LoopTurnRow, DecisionRow, CapabilityGrantRow, DelegationHopRow, ApprovalRequestRow, ApprovalResponseRow, AdapterRegistrationRow, AdapterInstanceRow, ConformanceReportRow, ArtifactRow, ConfigGenerationRow, ActiveConfigGenerationRow, IdempotencyRecordRow, EventStreamHeadRow, OutboxEventRow. Accompanied by `New*`, `*Cas`, and `*Patch` types where the design names them. TEXT state columns use the foundation's `from_state_str` converters; integer enums use `from_wire`.
- `OutboxEventRow` carries `event_id`, `stream_key`, `sequence`, `event_type`, `payload`, `sensitivity`, `retention`, `correlation_id`, `causation_id`, `created_at_ms`, `journal_published_at_ms`, `live_published_at_ms`, mirroring the DDL.
- `KernelStore::begin_write`/`begin_read` return `Box⟨dyn ... + '_⟩` borrowing the store; `commit`/`rollback` consume `Box⟨Self⟩`. Add a compile-time object-safety test that performs a no-op call through `Box⟨dyn KernelTxn⟩`.
- `testkit/src/store.rs` implements the full contract in memory: a `MockStore` with an internal map of rows per entity, cloning semantics for transaction isolation, CAS checks mirroring storage conditions, idempotency replay, and stream allocation. It must compile against the same traits and pass `tests/store_mock.rs`, which performs a representative run+effect+outbox+idempotency mutation inside one mock transaction and asserts atomic visibility.
- `testkit/src/lib.rs` gains `pub mod store;`. No production crate depends on testkit.

**Steps:**

- [ ] Write the object-safety compile test and the mock contract test first; confirm RED
- [ ] Implement `types.rs`, `txn.rs`, `repositories.rs`, `models.rs`, and `lib.rs` re-exports
- [ ] Implement the testkit mock until both tests pass
- [ ] Run `cargo test -p kernel-store -p testkit` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat(store): transaction contract and testkit mock [PST-002]`

**Acceptance criteria:**

- [ ] R2.1 / R2.5 — write transactions expose typed repos and no SQL; traits are object safe
- [ ] R2.2 — read transactions expose reads only (compile-time proof)
- [ ] R2.3 / R2.4 — mock commit is atomic and unreported drops leave no state
- [ ] R2.6 — the mock implements the identical trait objects used by production code
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store -p testkit` passes
- [ ] `cargo check --workspace` passes with the mock in place
- [ ] `cargo clippy -p kernel-store -p testkit --all-targets -- -D warnings` clean

---

### Task PST-003A: SQLite transactions and core repositories

- status: pending
- owner: -
- depends_on: PST-001, PST-002
- files: `agent-os/crates/kernel-store-sqlite/src/lib.rs`, `agent-os/crates/kernel-store-sqlite/src/txn.rs`, `agent-os/crates/kernel-store-sqlite/src/mapping.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/mod.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/runs.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/tasks.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/sessions.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/graph.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/environments.rs`, `agent-os/crates/kernel-store-sqlite/tests/cas.rs`, `agent-os/crates/kernel-store-sqlite/tests/rollback.rs`, `agent-os/crates/kernel-store-sqlite/tests/immutability.rs`, `agent-os/crates/kernel-store-sqlite/tests/contention.rs`
- requirements: R3.1, R3.2, R3.3, R3.4, R3.5, R3.6, P1, P2, N2
- scope: large
- model: capable

**Objective:** The SQLite store executes real transactions with writer ordering and epoch assertion, and its core repositories enforce CAS through database conditions.

**Context the implementer cannot infer:**

- `SqliteKernelStore` implements `KernelStore`. `begin_write` acquires a pooled connection, executes `BEGIN IMMEDIATE` as a raw statement, reads `daemon_fence` to assert `context.daemon_epoch` equals the persisted epoch (missing or mismatched → `FailedPrecondition`, `Never`), and returns a guard owning the connection. `commit` executes `COMMIT`; `rollback` executes `ROLLBACK`; `Drop` runs rollback and ignores its error. `begin_read` uses a plain read connection with no write statements. Use `sqlx` runtime queries (`query`, `query_as`) with explicit row mapping per the plan decision; statements are parameterized only.
- `mapping.rs` converts sqlx errors per the design error table: unique/PK → `Conflict`/`Never`; CHECK/FK/NOT NULL/trigger → `FailedPrecondition`/`Never`; busy/locked/timeout → `Unavailable`/`Safe`; unknown enum decode → `Internal`/`Never`; pool closed → `Unavailable`/`Safe`. Provide a helper that recognizes SQLite extended codes.
- Repos: implement `RunRepo`, `TaskRepo`, `SessionRepo`, `GraphRepo`, `EnvironmentRepo` from the port. CAS updates use `UPDATE ... WHERE key = ? AND expected columns` and return `Ok(rows_affected == 1)`. `GraphRepo::insert_dependency` checks reachability and expected revision inside the same immediate transaction; `EnvironmentRepo` has no update operations and relies on the schema triggers for immutability.
- Decode enum columns through the domain mirror enums; TEXT state columns through `from_state_str`; any unknown value fails the transaction with `Internal`.
- Tests: `cas.rs` (stale expectation returns false and mutates nothing), `rollback.rs` (fault after a successful write leaves zero rows; drop without commit rolls back), `immutability.rs` (update/delete on resolved environments is rejected by the store), `contention.rs` (N writers behind a barrier racing one CAS; exactly one wins; busy maps to `Unavailable` when the timeout is deliberately set to 1 ms). Use testkit barriers/fault points, never sleeps.
- `repos/mod.rs` declares the five core modules plus `pub mod` lines for the groups PST-003B, PST-004, and PST-005 will add as empty stubs referenced only when those files exist; coordinate by declaring only modules whose files you create, and let later tasks add their own lines.

**Steps:**

- [ ] Write `cas.rs` and `rollback.rs` failing first; confirm RED
- [ ] Implement `txn.rs`, `mapping.rs`, and the five repos; wire `lib.rs`
- [ ] Run the focused tests to GREEN, then add `immutability.rs` and `contention.rs`
- [ ] Run `cargo test -p kernel-store-sqlite` and the workspace gates
- [ ] Commit: `feat(store): sqlite transactions and core repos [PST-003A]`

**Acceptance criteria:**

- [ ] R3.1 — persisted enums decode through domain types and fail closed
- [ ] R3.2 / P2 — CAS conflicts are conflict reports, and a race produces exactly one winner
- [ ] R3.3 — writer ordering is reserved with `BEGIN IMMEDIATE` for head and claim mutations
- [ ] R3.4 — constraint and busy failures map to the stable codes in the design table
- [ ] R3.5 / P1 — failed or dropped transactions leave zero partial rows
- [ ] R3.6 — immutable records are rejected by the storage layer
- [ ] N2 — concurrency tests use barriers/fault points and pass `cargo fmt --check`
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store-sqlite` passes
- [ ] `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean

---

### Task PST-003B: Remaining entity repositories

- status: pending
- owner: -
- depends_on: PST-003A
- files: `agent-os/crates/kernel-store-sqlite/src/repos/effects.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/resources.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/timers.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/security.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/config.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/workspaces.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/adapters.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/artifacts.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/loop_turns.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/mod.rs`, `agent-os/crates/kernel-store-sqlite/tests/repos_remaining.rs`
- requirements: R3.1, R3.2, R3.3, R3.4
- scope: large
- model: capable

**Objective:** Every remaining entity group has typed repository operations with the same decode, CAS, and error-mapping discipline as the core group.

**Context the implementer cannot infer:**

- Implement the traits exactly as `design.md` lists them: `EffectRepo`, `ResourceRepo`, `TimerRepo`, `SecurityRepo`, `ConfigRepo`, `WorkspaceRepo`, `AdapterRepo`, `ArtifactRepo`, `LoopRepo`. Reuse `mapping.rs` and the decode helpers from PST-003A; do not duplicate mapping logic.
- Group-specific CAS expectations come from the schema and recipes: effects check `(state, executor_fencing_token)`; timers check `(state, version)`; config active pointer checks `expected_revision`; workspace leases check `(mode, epoch)`; loop turns check `state`.
- `AdapterRepo::insert_conformance_report` and `insert_registration` write immutable rows; `cas_instance_state` updates the durable adapter instance.
- `graph` and `runs` tables are not yours; if you need a helper from PST-003A, import it rather than copying.
- `repos/mod.rs` gains the nine `pub mod` lines; keep the file's existing order and formatting.
- `tests/repos_remaining.rs` exercises CRUD and one CAS conflict per group, including an unknown-value decode failure, using a temp database per test.

**Steps:**

- [ ] Write one failing CRUD+CAS test per group in `repos_remaining.rs`; confirm RED
- [ ] Implement the nine repositories and wire `repos/mod.rs`
- [ ] Run `cargo test -p kernel-store-sqlite --test repos_remaining` to GREEN
- [ ] Run the full crate suite and workspace gates
- [ ] Commit: `feat(store): remaining entity repositories [PST-003B]`

**Acceptance criteria:**

- [ ] R3.1 — all groups validate stored values through domain types
- [ ] R3.2 — every CAS reports conflicts without mutation
- [ ] R3.3 — reads and writes respect transaction isolation; no cross-transaction leakage
- [ ] R3.4 — constraint failures map to stable codes
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store-sqlite` passes
- [ ] `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean

---

### Task PST-004: Daemon lock, fence, and instance lifecycle

- status: pending
- owner: -
- depends_on: PST-003A
- files: `agent-os/crates/kernel-store-sqlite/src/fence.rs`, `agent-os/crates/kernel-store-sqlite/src/lib.rs`, `agent-os/crates/kernel-store-sqlite/tests/fence.rs`, `agent-os/crates/agentd/src/lock.rs`, `agent-os/crates/agentd/tests/lock_exclusion.rs`, `agent-os/crates/identity/src/lib.rs`, `agent-os/crates/identity/src/daemon.rs`
- requirements: R4.1, R4.2, R4.3, R4.4, R4.5, R4.6, N2
- scope: large
- model: capable

**Objective:** One daemon holds writer authority through an exclusive OS lock, and every write transaction is fenced by a durable epoch that survives restarts.

**Context the implementer cannot infer:**

- `DaemonLock::acquire(runtime_dir)` uses `std::fs::File::try_lock` (stable in Rust 1.94) on `agentd.lock`, creating it `0600`. `LockError::Held` carries the path. `release` drops the lock; process death releases it via the OS.
- `fence.rs` implements the fence operations by adding a `FenceRepo`-style module reachable from the store: claim (`INSERT ... ON CONFLICT DO UPDATE SET fencing_epoch = fencing_epoch + 1, instance_id = ?, lease_expires_ms = ? RETURNING ...`), read current, and assert. Claiming must happen in an immediate transaction so two processes serialize. `lib.rs` gains `pub mod fence;`.
- `identity::daemon::acquire(&store, lease_ms)` calls `acquire_daemon_fence` and returns `DaemonHandle { instance_id, fence }`. `identity/src/lib.rs` gains `pub mod daemon;`. The `DaemonInstanceId` comes from a caller-supplied `IdProvider`.
- Transaction epoch assertion is already implemented in PST-003A's guard; your fence test proves the stale case end to end by claiming a second epoch and replaying a transaction built with the old one.
- `agentd/tests/lock_exclusion.rs` uses the self-exec pattern: when env `LOCK_TEST_CHILD` is set, the test process acquires the lock, prints a ready line, and blocks on stdin; the parent test spawns `std::env::current_exe()` with that env, waits for ready, then asserts `DaemonLock::acquire` on the same directory returns `Held`. Second case: kill the child and assert the lock is acquirable and the epoch increments on the next claim.
- `runs.claim_daemon_epoch`, `effects.daemon_fencing_epoch`, and `timers.claim_daemon_epoch` exist in the schema; repositories already accept them in their patches. Nothing else persists daemon state (design D8).

**Steps:**

- [ ] Write `fence.rs` tests failing first (claim increments; stale txn rejected; restart increments); confirm RED
- [ ] Implement `fence.rs`, `DaemonLock`, and `identity::daemon`
- [ ] Write the two-process test and run it directly (not only in the suite)
- [ ] Run `cargo test -p kernel-store-sqlite -p agentd -p identity` and the workspace gates
- [ ] Commit: `feat(store): daemon lock and durable fence [PST-004]`

**Acceptance criteria:**

- [ ] R4.1 / R4.6 — a second process refuses; the first is unaffected and the database is untouched by the second
- [ ] R4.2 / R4.5 — claims are strictly increasing across restarts
- [ ] R4.3 — a stale-epoch transaction is rejected without mutation
- [ ] R4.4 — losing the fence refuses new transactions (documented and exercised with the stale path)
- [ ] N2 — the two-process test uses no sleeps; readiness is synchronized over the child's stdout
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store-sqlite --test fence` passes
- [ ] `cargo test -p agentd --test lock_exclusion` passes
- [ ] `cargo clippy -p kernel-store-sqlite -p agentd -p identity --all-targets -- -D warnings` clean

---

### Task PST-005: Idempotency, streams, and the outbox API

- status: pending
- owner: -
- depends_on: PST-003B, PST-004
- files: `agent-os/crates/kernel-store-sqlite/src/repos/idempotency.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/streams.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/mod.rs`, `agent-os/crates/kernel-store-sqlite/tests/idempotency.rs`, `agent-os/crates/kernel-store-sqlite/tests/outbox.rs`, `agent-os/crates/kernel-store-sqlite/tests/outbox_concurrent.rs`, `agent-os/crates/kernel-store-sqlite/tests/props.rs`, `agent-os/crates/events/src/lib.rs`, `agent-os/crates/events/src/outbox.rs`
- requirements: R5.1, R5.2, R5.3, R5.4, R5.5, R6.1, R6.2, R6.3, R6.4, R6.5, P2, P3, P4, N2
- scope: large
- model: capable

**Objective:** Commands can replay safely through persisted idempotency records, and every staged event gets a contiguous, durably committed stream position through a small staging API.

**Context the implementer cannot infer:**

- `IdempotencyRepo::lookup(principal, key)` returns the stored record; `insert` writes `(principal_id, idempotency_key, request_digest, outcome_code, outcome_payload, created_at_ms)`. The unique key is the pair; a same-key-different-digest insert must map to `Conflict` via the storage constraint, not a pre-check race.
- `StreamRepo::allocate(stream_key)` performs a single conditional statement against `event_stream_heads` (`INSERT ... ON CONFLICT DO UPDATE SET sequence = sequence + 1 RETURNING sequence`), so contiguity is structural. `insert_outbox(new)` writes the immutable row; unique `event_id` and unique `(stream_key, sequence)` are enforced by the schema. `scan_unpublished(limit)` orders by `stream_key, sequence` and filters on a null journal publication marker. `mark_published(event_id, kind)` updates only publication metadata.
- `events/src/outbox.rs` defines `DraftEvent` and `stage(txn, draft) -> Result⟨u64⟩` per `design.md`; `events/src/lib.rs` gains `pub mod outbox;`. `stage` allocates then inserts within the caller's transaction; it performs no I/O of its own.
- Tests: `idempotency.rs` (fresh insert, same-digest replay returns stored outcome, different digest conflicts, concurrent identical submissions produce one row), `outbox.rs` (contiguous sequences, duplicate event id rejected, ordered scan, atomic visibility with a failing transaction), `outbox_concurrent.rs` (M tasks staging bursts on one stream via barriers; sequences unique and contiguous), `props.rs` (P2 CAS race, P3 stream interleavings, P4 replay invariance using proptest).
- `repos/mod.rs` gains the two module lines. Keep formatting stable.

**Steps:**

- [ ] Write `idempotency.rs` and `outbox.rs` failing first; confirm RED
- [ ] Implement both repositories and the events staging API; wire `repos/mod.rs` and `events/src/lib.rs`
- [ ] Run focused tests to GREEN; add `outbox_concurrent.rs` and `props.rs`
- [ ] Run `cargo test -p kernel-store-sqlite -p events` and the workspace gates
- [ ] Commit: `feat(store): idempotency and transactional outbox [PST-005]`

**Acceptance criteria:**

- [ ] R5.1–R5.4 / P4 — lookup is transactional; replay returns the stored outcome with unchanged canonical rows; digest mismatch is a conflict
- [ ] R5.5 — racing identical submissions yield one insert
- [ ] R6.1–R6.3 — allocation is in-transaction, outbox rows are unique and immutable, scans are ordered
- [ ] R6.4 / P3 — concurrent appends are contiguous and unique
- [ ] R6.5 — outbox visibility matches the causing transaction's outcome
- [ ] P2 — CAS race property holds; N2 — no sleeps anywhere
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p kernel-store-sqlite -p events` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean

---

## Checkpoints

| After wave | Check | Command |
|---|---|---|
| 1 | Port compiles; bootstrap green; mock contract green | From `agent-os/`: `cargo test -p kernel-store -p testkit -p kernel-store-sqlite --test bootstrap` |
| 2 | Core repos, CAS, rollback, contention green | From `agent-os/`: `cargo test -p kernel-store-sqlite` |
| 3 | Remaining repos and fence green; two-process lock proven | From `agent-os/`: `cargo test -p kernel-store-sqlite -p agentd` |
| 4 | Public gates | From `agent-os/`: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; from the repo root: both validators |

## Rulings

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | PST-003 split into PST-003A/PST-003B behind one interface | Pre-authorized by plan assumption 7; keeps each task reviewable | Numbering differs from the pack; the umbrella ledger records it |
| 2 | PST-004 depends on PST-003A only, so it can run beside PST-003B | The fence needs the transaction guard, not the remaining repos | If a hidden dependency exists, PST-004 blocks and is re-sequenced |
| 3 | Draining state is in-memory only (design D8) | The persisted fence is the single authority | A future lifecycle module may want a persisted status; additive change |
