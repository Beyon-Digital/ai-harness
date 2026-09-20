# Task RES-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** RES-001 — Durable resource reservations + budget delegation
- **Status:** DONE
- **Commits:** `d9a9434` — `feat(resources): fenced reservations + parent budget delegation [RES-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `resources` crate: `ResourceUnit` (ChildRunSlots, SandboxSlots,
  ModelTokens, ModelCostMicrounits, WallClockMs, DiskBytes),
  `reserve` (amount > 0; idempotent on reservation id; transactional parent
  budget check — parent must be ACTIVE and same-unit, and
  sum(active same-unit children) + amount <= parent.amount, with `Unknown`
  counted as still-active → `ResourceExhausted`),
  `allocate`/`release` fenced by the reservation's fencing token,
  `mark_unknown`, `recover_unknown` (re-fences under a new token).
- `ResourceRead::list_children` added to kernel-store (trait + sqlite +
  mock) for the budget walk.
- Catalog events `ResourceReserved/Allocated/Released/Unknown` staged on the
  run stream.

## Evidence

`cargo test -p resources` — 5 tests incl. `concurrent_children_cannot_
exceed_parent` (10-way racing reservation against a parent budget commits
within the budget), over-delegation rejection, unit/state mismatch,
released-budget re-reservation, Unknown recovery by re-fencing.
