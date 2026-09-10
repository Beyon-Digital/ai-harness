> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Runtime Profiles

Profiles primarily bundle **run-scoped** defaults:

```yaml
profiles:
  local:
    sandbox: local-trusted
    workspace: local-worktree
    artifact_store: local-artifacts
    memory_store: sqlite-memory
    model_provider: openrouter
    tool_runtime: local

  isolated:
    extends: local
    sandbox: strong-container
    workspace: isolated-worktree

  cloud:
    sandbox: cloud-compute
    workspace: cloud-workspace
    artifact_store: s3
    memory_store: postgres-memory
```

AgentSpec selects a profile; authorized run creation may override compatible run-scoped bindings. The exact resolution is frozen into `ResolvedRunEnvironment`.

Daemon-wide kernel/services configuration is separate from runtime profiles.
