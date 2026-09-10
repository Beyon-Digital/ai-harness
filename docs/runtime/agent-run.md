> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# AgentRun

One concrete execution attempt. Key fields: run/task/session IDs, AgentSpec identity/version, parent/dependency relationships, state, recovery disposition, run revision, loop epoch, step sequence, event cursor, cancellation epoch, resolved environment ID, workspace lease, sandbox/resource handles, budgets/usage, timestamps, and terminal reason.

Retries create new AgentRuns under the same logical Task.
