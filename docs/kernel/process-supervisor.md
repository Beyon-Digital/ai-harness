> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Process Supervisor

Spawns and owns external loop/adapter/tool processes with explicit executable digest, environment allowlist, working directory, sandbox association, private IPC/bootstrap identity, stdout/stderr handling, resource limits, health probes, restart policy, and termination escalation.

A Python/Node crash must not crash `agentd`. Descendant process cleanup is mandatory for isolated/untrusted tiers.
