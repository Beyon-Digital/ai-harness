# Task RUN-002 — Dependency edges and cycle prevention

- **Status:** review
- **Agent:** agent-run002b
- **Commit:** `41e42da` — `feat(run-graph): dependency edges and cycle prevention [RUN-002]`
- **Depends on:** RUN-001 (done)
- **Files written:**
  - `agent-os/crates/run-graph/Cargo.toml`
  - `agent-os/Cargo.lock`
  - `agent-os/crates/run-graph/src/lib.rs`
  - `agent-os/crates/run-graph/src/graph.rs`
  - `agent-os/crates/run-graph/src/repository.rs`
  - `agent-os/crates/run-graph/tests/graph.rs`

## Acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| R3.1 — same task; target `Created` or unclaimed `Ready` | pass | `cross_task_edges_are_rejected_without_mutation` (`FailedPrecondition`/`Never`, no rows/head/event); `target_must_be_created_or_unclaimed_ready` (Created, Ready, Ready+expired claim accepted; Ready+live claim and 9 non-mutable states rejected) |
| R3.2 — no committed cycle | pass | `cycle_forming_edges_are_rejected` (3-chain then both closing orders, `Conflict`/`Never`, 2 rows / head 2 / 2 events, pairwise `is_reachable` acyclic); `self_edge_is_rejected_without_mutation`; reachability stays in `GraphRepo::insert_dependency`'s immediate transaction |
| R3.3 — head increment plus `DependencyAdded` in one transaction | pass | `edge_insertion_advances_head_and_stages_dependency_added` (head 0→1, one row at revision 0, one `DependencyAdded` on `task/{id}` sequence 1, internal/standard, payload decodes to the row and revision); `rollback_discards_the_edge_head_and_event` (rollback leaves head 0, no row, no event) |
| R3.4 — duplicate is deterministic without a second row | pass | `duplicate_edge_is_a_deterministic_no_op`: identical edge at the current head is `Ok` with no row/head/event change; same endpoints with another condition is `Conflict`/`Never`; a stale-revision duplicate is `Conflict`/`Never` |
| R3.5 — opposite-edge race commits at most one | pass | `opposite_edge_race_commits_at_most_one` (two barrier-released writers, exactly 1 commit + 1 `Conflict`/`Never`, one surviving edge, head 1, one event, acyclic) |
| R3.6 — ancestry from `runs.parent_run_id`, no second table | pass | `descendants_follow_parent_linkage_without_an_ancestry_table` (BFS `[child, sibling, grandchild]`; a dependency edge between siblings adds no child; missing root `NotFound`); `second_task_descendants_do_not_leak_across_task_boundaries` |
| P1 — committed graph acyclic under interleavings | pass | `sequential_inserts_keep_the_graph_acyclic` (proptest, 48 cases, up to 24 arbitrary pairs over 5 runs; `rows == head` and pairwise non-mutual reachability after every case); race test above |
| N2 — barriers, no sleeps; real store in a temp root | pass | every test uses `SqliteKernelStore` in a `tempfile::TempDir`; the race uses `tokio::sync::Barrier`; no `sleep` anywhere |
| Stale expected revision rejected | pass | `stale_expected_revision_is_rejected_without_mutation` (`Conflict`/`Never`, no mutation) |
| Missing endpoints rejected | pass | `missing_runs_are_not_found` (`NotFound`/`Never` for either endpoint) |
| `Unspecified` condition never persisted | pass | `unspecified_condition_is_rejected` (`FailedPrecondition`/`Never` before the store sees it) |
| No file outside `files:` changed | pass | `git show --stat 41e42da` lists exactly the six leased paths |
| No `unwrap`/`expect` in non-test source | pass | `graph.rs`/`repository.rs` contain none |

## Commands and output

### RED (`cargo test -p run-graph`, `tests/graph.rs` before implementation)

```
error[E0432]: unresolved import `run_graph::graph`
  --> crates/run-graph/tests/graph.rs:24:16
   |
24 | use run_graph::graph::add_dependency;
   |                ^^^^^ could not find `graph` in `run_graph`

error[E0432]: unresolved import `run_graph::repository`
  --> crates/run-graph/tests/graph.rs:25:16
   |
25 | use run_graph::repository::descendants;
   |                ^^^^^^^^^^ could not find `repository` in `run_graph`

error: could not compile `run-graph` (test "graph") due to 5 previous errors
```

The remaining three errors were one test-harness borrow-order bug (`ids` moved before
`PrincipalId::new(ids.as_ref())`) and two downstream type-inference errors caused by the
unresolved service imports; all were fixed before GREEN.

### GREEN (`cargo test -p run-graph`, from `agent-os/`)

```
running 14 tests
test cross_task_edges_are_rejected_without_mutation ... ok
test duplicate_edge_is_a_deterministic_no_op ... ok
test descendants_follow_parent_linkage_without_an_ancestry_table ... ok
test cycle_forming_edges_are_rejected ... ok
test edge_insertion_advances_head_and_stages_dependency_added ... ok
test rollback_discards_the_edge_head_and_event ... ok
test opposite_edge_race_commits_at_most_one ... ok
test missing_runs_are_not_found ... ok
test second_task_descendants_do_not_leak_across_task_boundaries ... ok
test self_edge_is_rejected_without_mutation ... ok
test unspecified_condition_is_rejected ... ok
test stale_expected_revision_is_rejected_without_mutation ... ok
test target_must_be_created_or_unclaimed_ready ... ok
test sequential_inserts_keep_the_graph_acyclic ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.31s
```

### `cargo test --workspace` (from `agent-os/`)

```
exit=0
89 "test result: ok" targets; grep for "test result: FAILED|panicked|error[" returns nothing
```

### `cargo clippy --workspace --all-targets -- -D warnings`

```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 44.62s
exit=0
```

### `cargo fmt --check`

```
exit=0 (workspace clean)
```

## Implementation notes

- **Manifest/lock.** `run-graph/Cargo.toml` adds `prost` to `[dependencies]` and
  `kernel-store-sqlite`, `tempfile`, `proptest` to `[dev-dependencies]` (append-only);
  `Cargo.lock` gains exactly those four edges for the `run-graph` package.
- **`repository.rs`.** `load_run` (`NotFound`), `has_live_claim` (expiry `> now_ms`),
  `is_mutable_target` (`Created`, or `Ready` without a live claim), `find_dependency`
  (pair lookup inside the task), and `descendants` — breadth-first over
  `runs.parent_run_id` from `RunRepo::list_by_task`, root excluded, deterministic by
  run-id order. No ancestry table is written or read.
- **`graph.rs`.** `add_dependency` validates in order: unspecified condition →
  both runs load → same task → mutable target → head exists and equals
  `expected_revision` → duplicate outcome → `GraphRepo::insert_dependency` (its
  immediate transaction re-checks reachability and advances the head) → stage
  `DependencyAdded` via `events::outbox::stage` with
  `CatalogClassificationPolicy::embedded()` (internal/standard) on `StreamKey::task`.
  The payload is prost-encoded `contract::RunDependency` carrying the persisted edge
  and `created_graph_revision = expected_revision`. `DependencyId` and `EventId` are
  minted from `domain::provider::SystemIdProvider`; no wall-clock or sleeps are used.
- **Reuse over reimplementation.** Cycle logic and the head CAS remain solely in the
  persistence `GraphRepo`; `graph.rs` never calls `is_reachable` or writes
  `run_graph_heads` itself.

## Concerns

1. **Id source.** The design signature for `add_dependency` carries no `IdProvider`,
   so `DependencyId`/`EventId` come from `SystemIdProvider` internally (same precedent
   as `EventBuilder` and `events::outbox`). Tests assert payload/persistence shape and
   identity between the row and the event, not fixed ids.
2. **Duplicate semantics are one of the two allowed outcomes.** Identical
   `(source, target, condition)` at the current head is an idempotent `Ok` (no row,
   head advance, or event); the same pair with a different condition, or any duplicate
   at a stale expected revision, is `Conflict`/`Never`. Two interleaved writers adding
   the same edge are serialized by the head CAS and surface as a store `Conflict`.
3. **Property case budget.** The P1 proptest opens one real SQLite store per case
   (48 cases) so the `run-graph` suite takes ~16–25 s; raising the case count trades
   CI time for more interleavings.
