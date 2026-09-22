# Agent OS MVP verification report

> Template for the artifact produced by `scripts/release_gate.sh`. Each
> release-gate run writes `artifacts/release-gate/verification-report.md`
> (this shape) plus `verification-report.json` (machine-readable twin).

- **verdict**: `PASS | FAIL`
- **git SHA**: `<commit>` (dirty files: `<n>`)
- **toolchain**: `<rustc -V output>`
- **platform**: `<uname -s> <uname -m>`
- **started**: `<UTC timestamp>`

## Gate checks

| check | status | seconds |
|---|---|---|
| buildpack_validator | pass | _ |
| repo_invariants | pass | _ |
| contract_mirror_lock | pass | _ |
| cargo_fmt | pass | _ |
| cargo_clippy | pass | _ |
| cargo_build | pass | _ |
| workspace_tests | pass | _ |
| concurrency_suite | pass | _ |
| security_suite | pass | _ |
| property_suite | pass | _ |
| e2e_suites | pass | _ |
| blocking_mvp_markers | pass | _ |
| test_sleep_hygiene | pass | _ |

## MVP exit criteria

Every row maps one `MVP_EXIT_CRITERIA.md` clause to the check that proved it
in this run. A criterion is `pass` only when the gate that exercises its
evidence ran and passed in this invocation — the gate never reports a
skipped suite as pass; a `--fast` run omits `cargo_build` but keeps every
evidence-bearing suite.

| # | criterion | evidence |
|---|---|---|
| 1 | single writer authority | kernel-store-sqlite fence tests + agentd lock_exclusion |
| 2 | idempotent command replay | kernel-store-sqlite idempotency + command-coordinator tests |
| 3 | txn/outbox atomicity | kernel-store-sqlite outbox tests |
| 4 | publisher re-run safe | events dispatcher + outbox_concurrent |
| 5 | no premature cursor | event-journal-sqlite journal + events live_bus |
| 6 | no dependency cycles | concurrency raced_graph + property dependency_graph |
| 7 | single claim winner | concurrency raced_ready_run_claims |
| 8 | cancel/spawn race | e2e_children_workspace::e2e_cancel_spawn_race |
| 9 | stale decision rejected | property run_transitions + runtime loop_turn tests |
| 10 | single effect fencer | concurrency raced_effect_claim/commit + property committed_effects |
| 11 | crash -> reconcile or Unknown | e2e_effect_recovery crash/blocked tests |
| 12 | timer fire/cancel single winner | concurrency raced_timer + property timer_version |
| 13 | descendants <= budget | concurrency raced_reservation + property reservation_descendants |
| 14 | no capability amplification | security confused_deputy tests |
| 15 | approval digest mismatch rejected | security approval_digest_binding |
| 16 | T2 cannot fall back to T0 | security t2_requirement |
| 17 | exclusive lease single writer | concurrency raced_exclusive_transfer + property never_two_active |
| 18 | fork/merge | e2e_children_workspace::e2e_parallel_fork_merge |
| 19 | frozen per-run bindings | config-engine service_binding_change_requires_restart |
| 20 | frozen global binding rejected | config-engine activation_cas_one_winner |
| 21 | handshake identity checks | security forged_adapter_hello + inbound_frame_order |
| 22 | drain-first shutdown | e2e_basic::e2e_idle_shutdown |
| 23 | end-to-end fixture flow | e2e_basic::e2e_complete_run_via_daemon_api |
| 24 | all quality gates | this gate's own check list |
