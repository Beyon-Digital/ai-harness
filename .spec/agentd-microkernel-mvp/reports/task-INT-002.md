# INT-002 — Hierarchical children and workspace delegation end-to-end

## What was built / verified

`crates/agentd/tests/e2e_children_workspace.rs` — three e2e tests over a real daemon + `agentctl` on the UDS API plus the workspace coordinator (`fork_for_child`, `acquire_lease`, `transfer_exclusive`, `verify_write_authority`, `merge`) against live git repos:

- `e2e_parallel_fork_merge` — parent's fixture loop emits two `spawn_agent` decisions; both children are created via the real `CreateTaskRun` path and drive to `Completed` on their own scripts. `fork_for_child` hands each child an isolated worktree pinned to the parent's recorded `base_revision` (asserted via `git rev-parse` and `workspaces.parent_workspace_id`/`base_revision` rows). Children write disjoint files, parent merges each fork cleanly (`merge --no-commit --no-ff`, commit between merges). A third fork + a parent-side edit to the same file produces a `Conflict` error and `merge --abort` leaves no partial state.
- `e2e_exclusive_transfer_stale_lease_rejected` — parent holds `EXCLUSIVE_WRITE`; `transfer_exclusive` moves the lease to a child run with a bumped epoch; the stale parent token fails `verify_write_authority` (`FailedPrecondition`) while the child's live token passes.
- `e2e_cancel_spawn_race` — parent parks at `WaitingHuman` (`request_approval` script), then `CancelRun` and a child `CreateTaskRun` race via `tokio::join!`. Both orders verified consistent: spawn-first → child row exists, parks at its own `request_approval`, subtree cancellation sweeps it to `Cancelled`; cancel-first → create rejected on stale `observed_parent_cancellation_epoch`, child row never exists. Parent always ends `Cancelled`.

## Real bug found and fixed

- `agentctl cancel-run` sent `expected_run_revision = u64::MAX` when `--expected-revision` was omitted; the runtime maps `0` to "no check", so every unflagged cancel silently failed its revision CAS. Fixed to `unwrap_or(0)` matching the other revision-gated commands (`crates/agentctl/src/lib.rs`).
- `create-run` idempotency key (earlier in this wave) now hashes all salient flags so flag-only invocations with different `--run-id`/`--parent-run-id` don't replay-collide.

## Tests (required by the task)

- `cargo test -p agentd --test e2e_children_workspace` → 3 passed (ran 3× consecutively for the race test).
- `cargo test --workspace` → 128 test binaries, 0 failures.
- `cargo clippy --workspace --all-targets -D warnings` clean; `cargo fmt --check` clean.

## Non-blocking follow-ups (outside this task)

- `DecisionInstruction::InvokeEffect` in the run worker still returns "not wired" — INT-003 scope.
- Concurrency here is `tokio::join!` on two agentctl calls, not a hammered barrier loop — INT-004 owns the heavy iteration counts.
