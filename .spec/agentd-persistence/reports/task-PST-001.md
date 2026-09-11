# Task PST-001 — Bootstrap the kernel database

- **Status:** DONE_WITH_CONCERNS
- **Owner:** agent-pst001
- **Commit:** `8514563` — `feat(store): versioned kernel.db bootstrap [PST-001]`
- **Date:** 2026-09-12
- **Depends on:** PST-000 (satisfied)

## Implementation notes

Files (lease-scoped, nothing else touched):

| File | Role |
|---|---|
| `agent-os/crates/kernel-store-sqlite/src/lib.rs` | `StoreConfig`, `SqliteKernelStore::{open, path}`, runtime-dir/file preparation |
| `agent-os/crates/kernel-store-sqlite/src/schema.rs` | `include_str!` of the inception schema, verbatim `raw_sql` bootstrap, version/table/PRAGMA verification |
| `agent-os/crates/kernel-store-sqlite/tests/bootstrap.rs` | 8 integration tests covering R1.1–R1.5 |

Bootstrap sequence in `SqliteKernelStore::open` (`src/lib.rs:39`):

1. Resolve the runtime directory as `config.path.parent()`; reject a path with no
   parent (`FailedPrecondition`/`Never`). Create the directory recursively and
   assert mode `0700` (`src/lib.rs:90`).
2. Create the database file with `OpenOptions::create_new(true).mode(0o600)` when
   absent and assert mode `0600` on every open (`src/lib.rs:109`). An
   `AlreadyExists` result means the file pre-existed; a non-regular path is
   rejected.
3. When *and only when* this open created the file, execute
   `agent-os/schema/kernel_store.sql` verbatim through `sqlx::raw_sql` on a
   dedicated connection built from the same `SqliteConnectOptions`
   (`src/schema.rs:49`). The schema is embedded with `include_str!`
   (`src/schema.rs:9`) and the schema file itself was not edited.
4. Open the pool with `create_if_missing(false)`, `busy_timeout` from
   `StoreConfig`, `journal_mode=WAL`, `synchronous=FULL`, `foreign_keys=ON`
   (`src/lib.rs:59`).
5. Verify on every open (`src/schema.rs:82`):
   - `kernel_meta.schema_version` is exactly the text `1`; absent or different
     values fail closed with `FailedPrecondition`/`Never`, naming the value found
     (`src/schema.rs:88`);
   - the table inventory equals the 30 schema tables exactly
     (`src/schema.rs:116`);
   - `foreign_keys=1`, `journal_mode=wal`, `synchronous=2`, `busy_timeout` equals
     the configured value (`src/schema.rs:149`).

`lib.rs` stays minimal (struct, `open`, `path`, `pub mod schema`); no repository
code or SQL escape hatch was added.

## Acceptance criteria

| Criterion | Evidence | Command |
|---|---|---|
| R1.1 fresh bootstrap creates the DB from the schema verbatim and seeds version 1 | `fresh_bootstrap_creates_database_from_schema`: 30 tables, version `1`, known tables present | `cargo test -p kernel-store-sqlite --test bootstrap` |
| R1.2 reopen performs no structural change (no `CREATE` executed) | `reopen_is_structurally_stable`: sqlite_master identical across reopen; an index dropped out-of-band is **not** re-created | same |
| R1.3 wrong or missing version fails closed naming the value | `wrong_schema_version_fails_closed` (message contains `2`), `missing_schema_version_fails_closed` (message contains `missing`); both `FailedPrecondition`/`Never`; DB not rewritten/re-seeded | same |
| R1.4 no repository or open path auto-creates tables | schema is only executed when this open created the file; proven by R1.2 test | same |
| R1.5 / N1 PRAGMAs and `0600`/`0700` modes verified | `restrictive_modes_are_enforced_at_open`, `missing_runtime_directory_is_created_0700`, `fresh_bootstrap_...` (journal_mode), `schema_constraints_are_enforced` (CHECK/UNIQUE/FK), plus startup `verify_pragmas` | same |
| G2 pack validators still pass; no pack file changed | both validators OK; `git diff` shows no schema/pack changes | `python3 tools/validate_repo.py`, `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` |
| No file outside `files:` changed | commit `8514563` contains only the three leased paths | `git show --stat 8514563` |

Additional boundary test: `path_without_parent_directory_is_rejected`.

## RED / GREEN

RED (`cargo test -p kernel-store-sqlite --test bootstrap`, before implementation):

```
error[E0432]: unresolved imports `kernel_store_sqlite::SqliteKernelStore`, `kernel_store_sqlite::StoreConfig`
  |                           no `StoreConfig` in the root
  |                           no `SqliteKernelStore` in the root
error: could not compile `kernel-store-sqlite` (test "bootstrap") due to 15 previous errors
```

The first build attempt was additionally blocked for ~45 s by the parallel
PST-002 agent's mid-flight `kernel-store` edits; once that crate compiled, the RED
above was obtained.

GREEN (`cargo test -p kernel-store-sqlite --test bootstrap`):

```
running 8 tests
test path_without_parent_directory_is_rejected ... ok
test fresh_bootstrap_creates_database_from_schema ... ok
test restrictive_modes_are_enforced_at_open ... ok
test missing_runtime_directory_is_created_0700 ... ok
test missing_schema_version_fails_closed ... ok
test reopen_is_structurally_stable ... ok
test wrong_schema_version_fails_closed ... ok
test schema_constraints_are_enforced ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.55s
```

Full crate and workspace gates:

- `cargo test -p kernel-store-sqlite` — 8 passed, 0 failed (plus empty doc-tests).
- `cargo clippy -p kernel-store-sqlite --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` (from `agent-os/`) — exit 0.
- `python3 tools/validate_repo.py` — `OK: 250 markdown, 13 canonical ports, no link/schema/catalog errors`.
- `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` — `BUILD PACK OK: 59 tasks, 111 markdown files, contracts locked`.

## Concerns

1. **`StoreConfig.path` interpretation.** The design fixes the field name but not
   its semantics. This implementation treats `path` as the `kernel.db` **file**
   path (its parent is the runtime directory), matching `create_if_missing(false)`
   and `path()` returning the same value. If later tasks intend `path` to be the
   runtime *directory*, downstream test setup must use
   `runtime_dir.join("kernel.db")`; `open` will otherwise fail closed rather than
   guess. No design decision was changed.
2. **Passed-through runtime-directory chmod.** Per R1.5/N1, every open re-asserts
   `0700` on `config.path.parent()`. Operators must point `AGENTD_HOME` at a
   dedicated runtime directory, not a shared parent.
3. **Partial-bootstrap safety.** The schema contains `PRAGMA journal_mode = WAL`,
   which SQLite refuses inside a transaction, so the verbatim script cannot be
   wrapped in one transaction. A crash mid-bootstrap leaves a file that fails
   closed on the next open (missing version or missing tables), never a silently
   repaired or forked database.
4. **PRAGMA test coverage.** `foreign_keys`/`synchronous`/`busy_timeout` are
   per-connection, so external test connections cannot observe the store's pool
   settings; they are asserted by `schema::verify` during every `open` instead,
   and FK behavior is additionally probed in `schema_constraints_are_enforced`.
