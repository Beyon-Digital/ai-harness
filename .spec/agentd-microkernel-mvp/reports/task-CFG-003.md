# Task CFG-003 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** CFG-003 — ResolvedRunEnvironment: freeze exact bindings atomically at run bind
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `config-engine/src/profile.rs`: profile `extends` resolution —
  root-first flattening, child keys win, cycles/missing parents rejected.
- `kernel-store`: `AdapterRead::list_registrations` feeds the resolver.
- `runtime/src/resolved_environment.rs`: `plan_environment` resolves
  the active generation's profile to exact adapter
  id/version/digest/capabilities per bound port (builtins carry their
  identity in the dedicated environment columns) and records
  AgentSpec/loop digest/config generation/workspace base+access/
  kernel+protocol versions/grants+approvals; `freeze_environment`
  inserts the environment + bindings and CAS-pins the run's
  `resolved_environment_id` in one transaction. Schema triggers make
  environment + bindings tables update/delete-proof.

## Evidence

`cargo test -p runtime --test resolved_environment` (4 tests): run
bound under G1 retains G1 bindings after G2 activates; duplicate
environment insert rejected; registered adapter produces a frozen
`resolved_bindings` row; a profile absent from the active generation
fails run start with `NotFound` and no environment row.
