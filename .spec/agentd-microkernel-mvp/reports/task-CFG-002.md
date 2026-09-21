# Task CFG-002 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** CFG-002 — Config generations: propose/validate/smoke-test, CAS activation, restart-required gating
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `kernel-store`: `ConfigRepo::set_generation_states` advances
  `validation_state`/`test_state` (COALESCE per-column, forward-only).
- `config-engine/src/generations.rs`: `propose` (digest dedupe →
  Conflict), `validate` (schema parse → validated|rejected),
  `smoke_test` (adapter binding refs must be builtin or registered →
  passed|failed) — all without moving the pointer.
- `config-engine/src/activate.rs`: `activate` CASes the singleton
  `active_config_generation` at an expected revision; eligibility
  requires `validated`+`passed`; a generation whose `services:` block
  differs from the running daemon's bindings fails
  `DAEMON_RESTART_REQUIRED` without moving the pointer; `rollback`
  reactivates a prior known-good generation through the same checks.

## Evidence

`cargo test -p config-engine` generation suite: untested generation
cannot activate; CAS race has exactly one winner; service-binding
change returns `DAEMON_RESTART_REQUIRED` with the pointer unmoved;
failed smoke leaves the pointer on the last known-good; rollback
restores G1 at revision 3.
