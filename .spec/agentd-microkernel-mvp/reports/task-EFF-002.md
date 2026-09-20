# Task EFF-002 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** EFF-002 — EffectRecord transitions + preparation
- **Status:** DONE
- **Commit:** `b4c0b31` — `feat(effects): contract resolver, preparation, executor fencing [EFF-001][EFF-002][EFF-003]`
- **Branch:** `devin/1789940928-effects-wave`

## What was implemented

- `record.rs` — `can_transition` legal-transition table:
  Prepared->{Claimed,Cancelled}; Claimed->{Claimed(reclaim),Dispatched,Cancelled};
  Dispatched->{Dispatched(redispatch),Acknowledged,Failed,Cancelled,Unknown};
  Acknowledged->{Committed,Failed}; Unknown->{Committed,Failed,Prepared};
  terminals Committed/Failed/Cancelled. `is_terminal`, `is_in_flight`.
- `coordinator.rs` — `PrepareRequest`/`PrepareOutcome{Prepared,Replayed}`;
  `prepare_effect` inserts `state=Prepared` inside the command write txn;
  duplicate logical identity (run_id, decision_id, operation, request_hash)
  replays the committed row instead of a second insert; `EffectPrepared` staged
  via the transactional outbox (`stream effect/<id>`).
- `kernel-store-sqlite/repos/effects.rs` — insert/get/transition/list repo.

## Evidence — task ACs

- No dispatch without committed Prepared: `claim` refuses non-prepared rows
  (NotFound); `dispatched_requires_committed_prepared` test.
- Prepare rollback test: `prepare_rollback_leaves_no_row_or_event`.
- Duplicate logical identity: `duplicate_logical_identity_replays_committed_row`.
- Illegal transitions rejected with FailedPrecondition.

`cargo test -p effects` — integration suite green (12 tests total).
