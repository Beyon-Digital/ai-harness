# Task SCH-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** SCH-001 — Durable one-shot scheduler timers
- **Status:** DONE
- **Commits:** `db62fe7` — `feat(scheduler): durable timers + claim/fence + runtime handlers [SCH-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `scheduler` crate over the `timers` table: `schedule` (idempotent on
  timer_id, `Conflict` on divergent payload), `cancel` (CAS
  `Scheduled->Cancelled`, replay-idempotent), `claim` (CAS with claim owner +
  fencing token = version+1 + daemon epoch), `claim_next_due` (re-claims
  `Claimed` rows whose owner epoch is stale — restart-safe reclamation),
  `mark_fired` (CAS `Claimed->Fired` at the post-claim version).
- Runtime command handlers `ScheduleTimer` / `CancelTimer` registered in
  `register_handlers`; staged catalog events `TimerScheduled`,
  `TimerClaimed`, `TimerFired`, `TimerCancelled` on the run stream.
- `agentd::workers::scheduler::SchedulerWorker`: polls `claim_next_due`,
  dispatches through `CommandCoordinator` with deterministic idempotency key
  `timer.fire.{timer_id}` (re-dispatch dedupes at the coordinator), then
  `mark_fired`. The worker never mutates run state directly.

## Evidence

`cargo test -p scheduler` — 6 tests: idempotent schedule + conflict, claim
vs cancel race (both directions), version-mismatch `Conflict`, double-fire
prevented, restart reclaim under a second epoch, not-due skip.
`cargo test -p runtime` — handler registration; workspace suite green.
