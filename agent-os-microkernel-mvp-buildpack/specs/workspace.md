> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Workspace Coordinator

## Durable record

Workspace identity is the `workspaces` row in `kernel-store-schema.sql`: `workspace_id`, `kind` (`git-worktree` or copy-on-create fork), `base_revision` (the exact revision the workspace was created from), `parent_workspace_id` for fork lineage (self-FK), and `created_at_ms`. `workspace_leases.workspace_id` references this identity; lease epochs and the transfer rules below are unchanged.

## Access modes

- `READ_ONLY`.
- `EXCLUSIVE_WRITE`.
- `ISOLATED_FORK`.
- `SHARED_COORDINATED_WRITE` — unsupported by the MVP local adapter unless locking/version-check capability is implemented and tested.

## Exclusive transfer

A parent may transfer exclusive write authority to a child.

Transaction:

1. verify parent owns active lease at expected lease epoch;
2. verify child is in the same authorized lineage and received delegated workspace-write capability;
3. advance lease epoch and owner;
4. mark old authority revoked in kernel metadata;
5. emit event.

**Enforcement requirement:** the workspace/sandbox layer must ensure the old writer no longer retains writable handles/mounts. For the T0 local adapter, exclusive-transfer tests use coordinator-mediated writes; the adapter must clearly report that raw host-process access cannot be a strong security boundary. Strong lease enforcement is required before advertising T2+.

## Parallel children

Default write-capable parallel delegation uses `Fork`:

```text
parent worktree/base revision
  ├─ child-A worktree
  └─ child-B worktree
```

Each child receives exclusive authority to its fork. Child output returns by explicit merge/cherry-pick/apply operation. No child is isolated from useful parent scope: the fork starts from the parent's exact base revision and authorized files.

## Local adapter

Use Git worktrees when the workspace is a Git repository. For non-Git workspaces, use copy-on-create directory fork for MVP tests and label the capability accordingly.
