# Plan — agentd-persistence

**Status:** draft
**Date:** 2026-09-11

Module spec. Phase 1 input is the approved umbrella plan at
`.spec/agentd-microkernel-mvp/plan.md` (capability map, build order, rulings). This module is
second in that build order; the foundation module is complete (14/14 tasks, approved branch).

## Problem statement

The foundation is green but nothing persists. The build pack's persistence requirements are
normative (`specs/kernel-store.md`, `specs/transaction-recipes.md`, `specs/kernel-store-schema.sql`)
and the schema is complete and validated, but no runtime code creates `kernel.db`, enforces the
schema version, exposes a transaction contract, implements repositories or CAS, holds the daemon
write lock, or provides idempotency and outbox primitives. Every later module — command
coordinator, events, runtime, effects, security — blocks on this module.

## Outcome

`KernelStore` is real: a versioned, PRAGMA-enforced SQLite database bootstraps exactly once;
object-safe async transaction and read interfaces are available to the command coordinator
without any SQL escape hatch; typed repositories implement create/read/CAS/immutability
behaviour; one daemon owns writer authority through an OS lock plus a durable fencing epoch;
idempotency records and the transactional outbox are safe under concurrent commands. Tasks
PST-001 through PST-005 are implemented and reviewed.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | `sqlx` is used with runtime-checked queries (`query`/`query_as` plus explicit row mapping), not compile-time macros | Compile-time macros require `DATABASE_URL` or committed `.sqlx` metadata at build time and change PST-002/003 ergonomics |
| 2 | `BEGIN IMMEDIATE` is issued as an explicit statement on a connection held by the transaction guard; commit/rollback consume the guard | A first-class sqlx API would simplify the wrapper; the semantics and tests stay the same |
| 3 | The daemon lock uses `std::fs::File::try_lock` (stable since Rust 1.89), adding no dependency | Older toolchains or missing platform support force a small crate like `fs2` and a workspace dependency addition |
| 4 | Runtime data lives under an `AGENTD_HOME` root (tests use a temp dir); default is the macOS application-support directory, with `kernel.db` at mode `0600` and its directory `0700` per `specs/limits.yaml` | Config keys, tests, and the daemon bootstrap path change |
| 5 | The SQLite pool allows a bounded number of connections (default 5) with `busy_timeout` from the normative PRAGMAs; correctness comes from SQLite transactions, never an application mutex | A single-writer connection model changes throughput and the contention test shape |
| 6 | PST-004 owns `crates/identity/src/daemon.rs` and PST-005 owns `crates/events/src/outbox.rs` even though those crates are otherwise later modules' territory | Later identity/events specs must not re-own those files, or ownership moves with a dependency edge |
| 7 | All repository groups named by `specs/kernel-store.md:21` land in PST-003 as the pack specifies | If it proves too large, it splits into PST-003a/003b behind the same interface, without changing the contract |

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| The transaction contract shape is already fixed | `agent-os-microkernel-mvp-buildpack/specs/kernel-store.md:12-21` | PST-002 implements `begin_write`/`begin_read`/`acquire_daemon_fence` and boxed `KernelTxn`/`KernelReadTxn`; no design invention required |
| Version rule and schema authority are fixed | `specs/kernel-store.md:23-25` | PST-001 executes the schema verbatim, seeds `kernel_meta(schema_version) = '1'`, fails closed otherwise |
| Storage invariants are enumerated | `specs/kernel-store.md:55-64` | Repository behaviour in PST-003/005 must enforce every listed invariant through DB conditions |
| Linearization recipes are normative | `specs/transaction-recipes.md` (Recipes A, B, F, H, I especially) | PST-003 CAS/claim primitives and PST-005 idempotency/outbox follow these sequences exactly |
| The schema already carries every table, CHECK, trigger, and PRAGMA | `specs/kernel-store-schema.sql`; counts verified in the foundation final review (30 tables, 15 CHECKs, 11 triggers) | PST-001 copies it into the workspace mirror and bootstraps; no schema edits expected |
| Workspace stubs and interfaces exist | `agent-os/crates/kernel-store/src/lib.rs`, `kernel-store-sqlite/src/lib.rs`, `identity/src/lib.rs`, `events/src/lib.rs` (pre-declared by FND-001) | Tasks fill bodies only; module roots stay single-owner |
| Errors, IDs, enums, testkit are ready | `agent-os/crates/errors`, `crates/domain`, `crates/testkit` (FND-003/004/005) | Repository errors map to `ErrorCode`/`RetryClass`; CAS uses typed states; concurrency tests use `ArmedFaults` barriers, never sleeps |
| `sqlx` is the chosen driver and its `BEGIN IMMEDIATE` gap is a known risk | umbrella `plan.md` assumption 7 and risk row; `DECISIONS.md:7` (D-003) | PST-002 resolves the transaction wrapper; PST-003 proves writer reservation under contention |
| File modes and pragmas are normative | `specs/limits.yaml` (`fs.*`), `architecture/persistence.md:14-18` | PST-001 applies them at bootstrap and asserts them in tests |
| Pack task briefs define expected outputs and tests | `tasks/PST-001.md` through `tasks/PST-005.md` | Tasks mirror the briefs one-to-one |

## Existing conventions to follow

- Build / test / lint from `agent-os/`: `cargo check --workspace`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Pack validators from the repo root: `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`,
  `python3 tools/validate_repo.py`.
- No `unwrap()`/`expect()` outside tests and proven startup invariants; typed `KernelError` only.
- No secrets in logs; classification via `observability` where records are emitted.
- Per-task evidence: commit with the task id, report under the spec's `reports/` directory,
  `TASK_STATUS.yaml` stays untouched (spec-flow owns this module's board).
- Everything authoritative comes from the pack specs; invent nothing.

## Approach

### Chosen

Mirror the pack's five persistence tasks one-to-one, in their dependency order, as the module's
internal DAG:

1. **PST-001** — bootstrap `kernel.db` from `agent-os/schema/kernel_store.sql` verbatim with the
   normative PRAGMAs and file modes, seed and verify `kernel_meta(schema_version)`, reject any
   other version, and prove tables, indexes, constraints, and triggers exist.
2. **PST-002** — define `KernelStore`, `KernelReadTxn`, `KernelTxn` as object-safe async traits
   with typed repository methods grouped exactly as `specs/kernel-store.md:21` lists, carrying the
   expected daemon fencing epoch, with consuming `commit`/`rollback`, and no SQL escape hatch.
3. **PST-003** — implement the SQLite transaction wrapper (`BEGIN IMMEDIATE` where writer
   ordering is required) and typed repositories with row encode/decode, enum validation, CAS
   helpers, stream-head allocation plus outbox insert in-transaction, and mapping of
   constraint/busy errors to stable codes.
4. **PST-004** — OS single-writer lock plus durable daemon fencing epoch: exclusive file lock on
   startup, epoch claim inside the store, persisted lease for diagnostics, assertion of the epoch
   in every write transaction, and refusal of authoritative work when lock or fence is lost.
5. **PST-005** — idempotency lookup/store keyed by principal and key with digest-conflict
   rejection, contiguous stream sequence allocation, immutable outbox rows with unique event id
   and stream position, and an ordered unpublished scan.

Dependency edges: PST-001 and PST-002 are independent roots; PST-003 needs both; PST-004 needs
PST-003; PST-005 needs PST-003 and PST-004.

### Rejected

| Alternative | Why not |
|---|---|
| `rusqlite` with a blocking adapter | The user chose `sqlx`; the umbrella plan records it as decision D10, and the transaction contract is async |
| Compile-time `sqlx::query!` macros | Forces `DATABASE_URL` or committed offline metadata into every build and CI job; runtime-checked queries keep the hermetic build the foundation established |
| A single global writer mutex with one connection | Repeats an authority the database already provides; `specs/kernel-store.md:68` forbids application-level exclusion as the source of truth |
| Auto-creating tables in repositories on demand | Explicitly forbidden: `specs/kernel-store.md:25`, PST-001 acceptance |
| Building the Event Journal in this module | The journal is a separate database and belongs to the events module; this module only allocates stream sequences and writes outbox rows |
| Starting the command coordinator here to prove transactions | The coordinator is the next module; PST-002's acceptance is proven by its mock in testkit plus PST-003 repositories |

## Scope

**In scope**

- PST-001 through PST-005 exactly as their briefs define them
- The pack-required tests: bootstrap/reopen/version, CRUD, rollback, CAS conflicts, concurrent
  writer contention, outbox/stream uniqueness, replay/conflict, two-process lock exclusion,
  stale-epoch rejection, concurrent sequence allocation
- The `agent-os/schema/kernel_store.sql` install (already mirrored by FND-002)

**Explicitly out of scope**

- Command Coordinator, event journal, and any runtime semantics above the repository layer
- Additional schema changes; if a gap is found it becomes an explicit pack edit, not a silent
  migration
- Transactions across databases (`kernel.db` and `events.db` are separate authorities)
- Performance tuning beyond the correctness requirements

## Capability map

Single capability — persistence. No decomposition needed; the five tasks above are its internal
DAG and share one interface surface.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| `sqlx` has no first-class `BEGIN IMMEDIATE`; a naive wrapper silently becomes deferred | Medium | Writer-ordering CAS tests could pass while real contention allows lost updates | PST-002 designs a guard that issues the statement explicitly; PST-003's contention test asserts writer reservation under load |
| Object-safe async traits with borrows (`KernelTxn` borrowing the store) hit lifetime friction | Medium | PST-002 churn; downstream interface drift | Compile-time object-safety test is the RED anchor; `async-trait` and boxed transaction guards per the pack shape |
| Concurrency tests become flaky on a laptop (WAL + busy timeout) | Medium | False failures, wasted review rounds | Use testkit barriers and fault points with deterministic interleavings; no sleeps; assert database invariants, not timing |
| Repository scope in PST-003 is large and tempts splitting mid-flight | Medium | Wave disruption | Assumption 7 pre-authorizes a clean split behind the same interface if the task overflows |
| PST-004/005 file ownership spills into later modules' crates | Low | Ownership collisions when identity/events specs start | Recorded as assumption 6 and as a ruling in this spec's ledger before implementation |
| Scope creep into coordinator semantics | Low | Duplicated or conflicting authority | Every task brief already forbids SQL escape hatches and coordinator logic |

## Parallelisation forecast

- **Wave 1:** PST-001 and PST-002 in parallel (fresh schema bootstrap versus pure interface
  definition; no shared files).
- **Wave 2:** PST-003 (needs both).
- **Wave 3:** PST-004 (needs the SQLite implementation).
- **Wave 4:** PST-005 (needs the store and the fence).
The graph is mostly sequential after wave 1; that is intrinsic — every later task builds on the
transaction implementation.

## Open questions for the user

1. **Query style.** Runtime-checked `sqlx::query_as` with explicit row mapping (recommended,
   keeps builds hermetic) or compile-time macros with committed offline metadata?
2. **Data location.** `AGENTD_HOME` environment root with a macOS application-support default
   (recommended, matches the testkit host conventions) or a repository-local runtime directory?
3. **Pool model.** Bounded pool of 5 with `busy_timeout = 5000` (recommended) or a single writer
   connection shared through the store?

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (recorded from explicit chat instruction)
**Date:** 2026-09-11
