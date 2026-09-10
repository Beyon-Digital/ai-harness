> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Hermes and Codex Loops Concurrently

Bind loops per AgentSpec/run, e.g. personal assistant uses Hermes while coding tasks use Codex. Both share kernel state/effects/security/RunGraph but may use different run-scoped model/context/memory/sandbox/workspace bindings. A Hermes parent may spawn a Codex child with explicitly delegated authority.
