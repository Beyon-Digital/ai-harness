# INT-004 — Concurrency races + seeded property suites

## Deliverables
- `crates/agentd/tests/concurrency/main.rs` — 7 multi-threaded, barrier-synced
  races against the real sqlite store (`#[tokio::test(flavor="multi_thread")]`,
  `tokio::sync::Barrier`; zero sleeps):
  1. `raced_ready_run_claims_single_winner` — 8 racers claim one `Ready` run;
     exactly 1 commits, `run_revision` bumps once (C7).
  2. `raced_graph_edge_insertions_never_cycle` — 8 lanes × 64 rounds racing
     opposite-direction edge inserts; final committed edge set verified acyclic
     by DFS (C6).
  3. `raced_timer_fire_cancel_single_winner` — fire-vs-cancel CAS from the same
     `(state, version)`; exactly one winner per round, persisted row matches the
     winner (C12).
  4. `raced_effect_claim_single_fencer` — 8 racers claim one `Prepared` effect;
     exactly 1 fencing token issued (C10).
  5. `raced_effect_commit_single_fencer` — real fencer vs forged token+1 racing
     dispatch→ack→commit; only the recorded executor commits (C10).
  6. `raced_exclusive_transfer_single_winner` — two transfers at the same lease
     epoch; exactly one commits, epoch bumps once (C17).
  7. `raced_reservation_children_never_exceed_parent_budget` — 8 racers × 8
     child reserves of 200 under a 1000-unit parent; granted total == 1000
     exactly, all losers fail `ResourceExhausted` (C13).

- `crates/agentd/tests/property/main.rs` — 8 seeded proptest properties
  (`cases: 512` default, `PROPTEST_CASES`/`PROPTEST_SEED` override, failures
  persist to `proptest-regressions/`):
  1. `run_transitions_legal_and_revision_monotonic` — random legal+illegal
     `runtime::run::transition` walks; every committed transition satisfies
     `allows`, revision strictly increases, terminal runs never move.
  2. `decision_tuple_unique_per_epoch_step_turn` — random `(epoch, step, turn)`
     decision inserts; `decision_id` uniqueness enforced, committed rows
     fetchable by key.
  3. `dependency_graph_never_contains_a_cycle` — random edges among 16 runs;
     DFS acyclicity checked after EVERY committed state.
  4. `committed_effects_never_regress` — random claim/dispatch/ack/commit/
     fail/cancel ops incl. forged executors and off-by-delta fencing tokens;
     every committed transition passes `can_transition`, `Committed` is
     absorbing.
  5. `reservation_descendants_within_parent_budget` — random delegate/
     allocate/release trees; active children sum ≤ parent amount at every
     committed read.
  6. `timer_version_monotonic_terminal_stable` — scheduler-legal
     claim/fire/cancel CAS ops with version deltas; version bumps exactly once
     per winner, terminal states never re-opened by a committed op.
  7. `never_two_active_exclusive_leases` — random acquire/revoke on one
     workspace; ≤1 active `ExclusiveWrite` lease at every committed read.
  8. `outbox_sequence_strictly_contiguous_per_stream` — random multi-stream
     allocate+insert; committed sequences per stream are exactly `1..=n`.

## Run
- `cargo test -p agentd --test concurrency` → 7/7 pass (~0.5s).
- `cargo test -p agentd --test property` → 8/8 pass (~83s at 512 cases).
- `cargo fmt --check` clean; `cargo clippy --workspace --all-targets` clean.

## Notes
- Raw `TimerRepo::cas_transition` is deliberately caller-checked: terminal
  states are not protected at the store primitive — the scheduler only ever
  addresses `Scheduled`/`Claimed` rows. The property therefore models the
  legal op set rather than asserting the primitive rejects illegal pairs.
- Property suite uses one `current_thread` runtime per case shared by every
  store call; sqlx connections must not outlive their runtime.
