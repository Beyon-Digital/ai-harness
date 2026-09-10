> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Glossary

**KernelStore** — authoritative transactional metadata store.  
**Outbox** — records durable events atomically with state mutations for later publication.  
**EffectRecord** — durable state of an external/costly mutation.  
**Unknown effect** — operation may have occurred but outcome cannot yet be established.  
**Fencing token** — monotonically newer authority preventing stale workers/daemons from committing.  
**ResolvedRunEnvironment** — immutable exact bindings/infrastructure metadata for one run.  
**Runtime Profile** — defaults for primarily run-scoped bindings.  
**WorkspaceLease** — persisted authority to access/write a workspace.  
**Adapter capability** — semantic feature supported by an implementation.  
**Security capability** — scoped authority granted to an actor/run.  
**Replay** — historical/state reconstruction, not guaranteed deterministic re-execution.
