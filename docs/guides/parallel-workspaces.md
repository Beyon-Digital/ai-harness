> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Parallel Subagents and Workspace Authority

Default parallel coding topology:

```text
Parent workspace @ commit X
├─ fork/worktree A → Child A EXCLUSIVE_WRITE
└─ fork/worktree B → Child B EXCLUSIVE_WRITE
```

Children retain the complete useful parent project scope but mutate isolated forks. Parent later merges/cherry-picks/applies outputs.

For sequential delegation, parent transfers its exclusive writer lease to a child and temporarily loses write authority until return. Coordinated concurrent shared writes require `SHARED_COORDINATED_WRITE` capability and adapter locking/version semantics.
