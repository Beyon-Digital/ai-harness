> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Workspace Coordinator

Owns logical workspace identity, leases, access modes, delegation, forks, and merge/apply operations.

## Access modes

- `READ_ONLY`.
- `EXCLUSIVE_WRITE` — one active writer lease for the physical workspace.
- `ISOLATED_FORK` — independently writable child workspace derived from a parent snapshot/commit.
- `SHARED_COORDINATED_WRITE` — concurrent writers only when the adapter advertises locking/version/conflict semantics.

## Parent-child rights

A parent may:
- delegate read authority;
- **transfer** exclusive write authority to one child, suspending its own writer lease until return;
- create isolated writable forks for parallel children;
- delegate coordinated shared write only if supported.

A child cannot delegate more authority than it received.

## Parallel coding default

Parallel write-capable children use isolated Git worktrees/forks by default, preserving full parent project scope while avoiding undefined concurrent mutation. Their output returns through explicit merge/cherry-pick/apply/reconciliation.

## Enforcing lease transfer

`EXCLUSIVE_WRITE` is not merely scheduler metadata. Before transferring the writer lease, the coordinator must prevent the previous holder from continuing write-capable I/O. Depending on adapter/trust tier this can require pausing write-capable tool execution, revoking/remounting the workspace read-only, closing/restarting a sandbox, or using adapter-native fencing/version checks.

An adapter that cannot enforce revocation against the relevant execution environment must advertise `enforced_write_lease=false`; it cannot satisfy workflows requiring strong exclusive-transfer semantics. T0 trusted local-process mode may provide cooperative exclusivity only and must say so explicitly.
