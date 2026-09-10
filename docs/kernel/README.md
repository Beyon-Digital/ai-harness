> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Rust Kernel Components

`agentd` hosts these trusted components in one process unless isolation is specifically required:

- [`agentd`](agentd.md)
- [Command Coordinator](command-coordinator.md)
- [KernelStore](kernel-store.md)
- [Runtime Manager](runtime-manager.md)
- [RunGraph Manager](run-graph-manager.md)
- [Effect Coordinator](effect-coordinator.md)
- [Identity / Lease / Fencing](identity-fencing.md)
- [Permission Engine](permissions.md)
- [Secrets Broker](secrets-broker.md)
- [Workspace Coordinator](workspace-coordinator.md)
- [Process Supervisor](process-supervisor.md)
- [Sandbox Manager](sandbox-manager.md)
- [Adapter Registry](adapter-registry.md)
- [Configuration Engine](config-engine.md)
- [Scheduler](scheduler.md)
- [Resource Accounting](resource-accounting.md)
- [Event System](event-system.md)
- [Recovery Coordinator](recovery.md)
- [Shutdown Coordinator](shutdown.md)
