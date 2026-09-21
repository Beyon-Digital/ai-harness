# Task WRK-003 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** WRK-003 — Workspace coordinator: epoch leases, transfer, fork/merge, capability-gated shared write
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `kernel-store`: `LeasePatch.lease_epoch` lets a transfer CAS check the
  stale epoch AND stamp the next epoch in one atomic update.
- `kernel-store-sqlite/src/repos/workspace_leases.rs`: lease decode +
  `get/insert/cas` impls; `cas_lease` writes `lease_epoch =
  COALESCE(?5, lease_epoch)` under the epoch preconditions.
- `workspace/src/coordinator.rs`: `acquire_lease` (RO/EW; exclusive
  uniqueness via the partial index), `transfer_exclusive` (expected
  epoch + delegated capability recorded; stale token loses), `revoke`,
  `verify_write_authority` (the write gate), `fork_for_child` (isolated
  fork + child-held exclusive lease), and `merge` (`git fetch
  <fork_root> <src_head>` + `--no-commit` merge; conflicts enumerated
  and aborted — explicit apply path surfacing conflicts).
  `SHARED_COORDINATED_WRITE` is rejected unless the adapter advertises
  `locking`+`versioned_write`+`conflict_report` capabilities. Raw host
  fds documented as NOT a security boundary.

## Evidence

`cargo test -p workspace` coordinator suite (5 tests): stale-token
rejection after transfer, exclusive-lease uniqueness, parallel forks
independent + merge clean/conflict surfacing (conflict file named),
shared mode rejected locally and granted for a capable adapter.
