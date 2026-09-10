> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Configuration Binding Scopes

`bootstrap-global`: KernelStore, root daemon identity, and root cryptographic/fencing material. These are selected by daemon deployment configuration and are not mutable through ordinary runtime config transactions.

`generation-global`: Event Journal, queue, Secret Store, remote Transport, and scheduler backing. One validated service generation is active at a time. Existing runs keep their persisted resolved bindings where the service participates in run execution.

`run-scoped`: loop, sandbox, workspace, artifact store, model provider, memory store, context service/strategy, tools, and other behavioral services. They are resolved at run start and frozen in `ResolvedRunEnvironment`.
